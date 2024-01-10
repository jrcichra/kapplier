use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::HeaderMap,
    routing::{get, post},
    Router,
};
use clap::Parser;
use kube::Client;
use log::{info, trace};
use prometheus::{FILE_APPLY_COUNT, RUN_LATENCY};
use std::{path::Path, process, time::Duration};
use tokio::{net::TcpListener, time::Instant};
use walkdir::WalkDir;
pub mod kubeclient;
pub mod prometheus;

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, env, default_value = "kapplier")]
    user_agent: String,
    #[clap(long, env, default_value = "repo")]
    path: String,
    #[clap(long, env, default_value = "")]
    subpath: String,
    #[clap(long, env, default_value = "true")]
    ignore_hidden_directories: bool,
    #[clap(long, env,default_values = ["yml", "yaml"])]
    supported_extensions: Vec<String>,
    #[clap(long, env, default_value = "300")]
    full_run_interval_seconds: u64,
    #[clap(long, env, default_value = "9100")]
    webserver_port: u16,
}

#[derive(Clone)]
struct AppState(Args, String);

async fn webhook(State(state): State<AppState>, headers: HeaderMap) -> String {
    info!("Got a webhook call with headers: {:?}", headers);
    let client = Client::try_default().await.unwrap();
    tokio::spawn(async move {
        info!("spawning webhook run");
        match reconcile(&state.0, &state.1, &client).await {
            Err(e) => {
                info!("reconcile error: {:?}", e);
            }
            Ok(_) => {}
        };
        info!("webhook run complete");
    });
    "ok".to_string()
}

#[tokio::main]
async fn main() -> Result<()> {
    // setup
    simple_logger::init_with_level(log::Level::Info)?;
    let args = Args::parse();
    let full_run_args = args.clone();
    let reconcile_args: Args = args.clone();
    let mut full_path = args.path.clone();
    if !args.subpath.is_empty() {
        full_path = format!("{}/{}", &args.path, &args.subpath);
    }

    // handle control c
    ctrlc::try_set_handler(move || {
        info!("received Ctrl+C! Exiting...");
        // exit immediately
        process::exit(0);
    })?;

    // web server for metrics
    let webserver_full_path = full_path.clone();
    let webserver_task = tokio::spawn(async move {
        let app = Router::new()
            .route("/metrics", get(prometheus::gather_metrics))
            .route("/webhook", post(webhook))
            .with_state(AppState(reconcile_args, webserver_full_path));
        let bind = format!("0.0.0.0:{}", args.webserver_port);
        let listener = TcpListener::bind(&bind).await.unwrap();
        info!("listening on {}", &bind);
        axum::serve(listener, app).await.unwrap();
    });

    let full_path_clone = full_path.clone();
    let full_run_task = tokio::spawn(async move {
        let args = full_run_args;
        // TODO: proper error handling
        let client = Client::try_default().await.unwrap();
        loop {
            tokio::time::sleep(Duration::from_secs(args.full_run_interval_seconds)).await;
            info!("starting full run");
            match reconcile(&args, &full_path_clone, &client).await {
                Err(e) => {
                    info!("reconcile error: {:?}", e);
                }
                Ok(_) => {}
            };
            info!("full run complete");
        }
    });

    // wait for threads to finish
    webserver_task.await?;
    full_run_task.await?;

    Ok(())
}

async fn reconcile(args: &Args, full_path: &str, client: &Client) -> Result<()> {
    let walker = WalkDir::new(full_path).sort_by_file_name().into_iter();
    for entry in walker {
        let entry = entry.context("could not unwrap entry")?;
        let path = entry.path();
        let path_str = path.to_str().context("could not convert path to str")?;
        if !should_be_applied(path, args) {
            continue;
        }
        // trigger a kubectl apply update
        let now = Instant::now();
        let res = kubeclient::apply(client.to_owned(), path_str, &args.user_agent).await;
        let elapsed = now.elapsed();
        let success = &res.is_ok().to_string();

        RUN_LATENCY
            .with_label_values(&[success, path_str])
            .set(elapsed.as_secs_f64());
        FILE_APPLY_COUNT
            .with_label_values(&[success, path_str])
            .inc();
        if res.is_err() {
            return res;
        }
    }
    Ok(())
}

fn should_be_applied(path: &Path, args: &Args) -> bool {
    let path_str = path.to_str().unwrap();

    if args.ignore_hidden_directories {
        if path
            .components()
            .find(|e| {
                let string = e.as_os_str().to_str().unwrap();
                if string.starts_with(".") && string.len() > 1 {
                    return true;
                }
                return false;
            })
            .is_some()
        {
            trace!("path is within hidden directory: {}", path_str);
            return false;
        }
    }

    // ignore files without the supported extension
    if let Some(extension) = path.extension() {
        if !&args
            .supported_extensions
            .contains(&extension.to_str().unwrap().to_string())
        {
            trace!("extension is ignored: {}", path_str);
            return false;
        }
    } else {
        trace!("no extension is ignored: {}", path_str);
        return false;
    }
    return true;
}
