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
use prometheus::{FILE_APPLY_COUNT, RECONCILE_DURATION_SECONDS, RECONCILE_FAILURE_COUNT, RUN_LATENCY};
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
    #[clap(long, env, default_values = ["yml", "yaml"])]
    supported_extensions: Vec<String>,
    #[clap(long, env, default_value = "300")]
    full_run_interval_seconds: u64,
    #[clap(long, env, default_value = "9100")]
    webserver_port: u16,
    /// Only apply documents that have this annotation. Format: key=value or just key to check presence.
    #[clap(long, env)]
    filter_annotation: Option<String>,
    /// Only apply documents that have this label. Format: key=value or just key to check presence.
    #[clap(long, env)]
    filter_label: Option<String>,
}

#[derive(Clone)]
struct AppState(Args, String);

async fn webhook(State(state): State<AppState>, headers: HeaderMap) -> String {
    info!("Got a webhook call with headers: {:?}", headers);
    tokio::spawn(async move {
        info!("spawning webhook run");
        let client = match Client::try_default().await {
            Ok(c) => c,
            Err(e) => {
                info!("webhook client error: {:?}", e);
                return;
            }
        };
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
    let webserver_port = args.webserver_port;
    let webserver_task = tokio::spawn(async move {
        let app = Router::new()
            .route("/metrics", get(prometheus::gather_metrics))
            .route("/webhook", post(webhook))
            .with_state(AppState(args, webserver_full_path));
        let bind = format!("0.0.0.0:{}", webserver_port);
        let listener = TcpListener::bind(&bind).await.unwrap();
        info!("listening on {}", &bind);
        axum::serve(listener, app).await.unwrap();
    });

    let full_path_clone = full_path.clone();
    let full_run_task = tokio::spawn(async move {
        let args = full_run_args;
        loop {
            tokio::time::sleep(Duration::from_secs(args.full_run_interval_seconds)).await;
            info!("starting full run");
            let client = match Client::try_default().await {
                Ok(client) => client,
                Err(e) => {
                    info!("full run client error: {:?}", e);
                    continue;
                }
            };
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
    let start = Instant::now();
    let discovery = kubeclient::run_discovery(client.clone()).await?;
    let filter = args.filter_annotation.as_deref();
    let filter_label = args.filter_label.as_deref();

    let mut total_failures: i64 = 0;
    let mut file_count = 0;

    let walker = WalkDir::new(full_path).sort_by_file_name().into_iter();
    for entry in walker {
        let entry = entry.context("could not unwrap entry")?;
        let path = entry.path();
        let path_str = path.to_str().context("could not convert path to str")?;
        if !should_be_applied(path, args) {
            continue;
        }
        file_count += 1;
        let now = Instant::now();
        let res = kubeclient::apply(client.to_owned(), &discovery, path_str, &args.user_agent, filter, filter_label).await;
        let elapsed = now.elapsed();
        let success = matches!(res, Ok(0)).to_string();

        if let Ok(failures) = &res {
            total_failures += failures;
        }

        RUN_LATENCY
            .with_label_values(&[success.clone(), path_str.to_owned()])
            .set(elapsed.as_secs_f64());
        FILE_APPLY_COUNT
            .with_label_values(&[success, path_str.to_owned()])
            .inc();
    }

    let elapsed = start.elapsed();
    RECONCILE_DURATION_SECONDS.set(elapsed.as_secs_f64());
    RECONCILE_FAILURE_COUNT.set(total_failures as f64);
    info!(
        "reconcile complete: {} files in {:.2}s, {} failures",
        file_count,
        elapsed.as_secs_f64(),
        total_failures
    );

    Ok(())
}

fn should_be_applied(path: &Path, args: &Args) -> bool {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return false,
    };

    if args.ignore_hidden_directories {
        if path
            .components()
            .find(|e| {
                let string = match e.as_os_str().to_str() {
                    Some(s) => s,
                    None => return false,
                };
                string.starts_with('.') && string.len() > 1
            })
            .is_some()
        {
            trace!("path is within hidden directory: {}", path_str);
            return false;
        }
    }

    // ignore files without the supported extension
    if let Some(extension) = path.extension() {
        let ext_str = match extension.to_str() {
            Some(s) => s,
            None => return false,
        };
        if !args.supported_extensions.contains(&ext_str.to_string()) {
            trace!("extension is ignored: {}", path_str);
            return false;
        }
    } else {
        trace!("no extension is ignored: {}", path_str);
        return false;
    }
    true
}
