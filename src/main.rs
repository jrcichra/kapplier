use anyhow::{Context, Result};
use axum::{routing::get, Router};
use clap::Parser;
use kube::{Client, Discovery};
use log::{info, trace};
use prometheus::{FILE_APPLY_COUNT, RUN_LATENCY};
use std::{path::Path, process, thread, time::Duration};
use tokio::{net::TcpListener, time::Instant};
use walkdir::WalkDir;
pub mod kubeclient;
pub mod prometheus;

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, default_value = "kapplier")]
    user_agent: String,
    #[clap(long, default_value = "repo")]
    path: String,
    #[clap(long, default_value = "")]
    subpath: String,
    #[clap(long, default_value = "true")]
    ignore_hidden_directories: bool,
    #[clap(long, default_values = ["yml", "yaml"])]
    supported_extensions: Vec<String>,
    #[clap(long, default_value = "300")]
    full_run_interval_seconds: u64,
    #[clap(long, default_value = "9100")]
    metrics_port: u16,
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
    let metrics_task = tokio::spawn(async move {
        let app = Router::new().route("/metrics", get(prometheus::gather_metrics));
        let bind = format!("0.0.0.0:{}", args.metrics_port);
        let listener = TcpListener::bind(&bind).await.unwrap();
        info!("listening on {}", &bind);
        axum::serve(listener, app).await.unwrap();
    });

    // wait for directory to exist
    info!("waiting for path to exist: {}...", &full_path);
    while !Path::new(&full_path).exists() {
        thread::sleep(Duration::from_secs(1));
    }

    let full_path_clone = full_path.clone();
    let full_run_task = tokio::spawn(async move {
        let args = full_run_args;
        // TODO: proper error handling
        let client = Client::try_default().await.unwrap();
        let discovery = Discovery::new(client.clone()).run().await.unwrap();
        loop {
            info!("starting full run");
            match reconcile(&args, &full_path_clone, &client, &discovery).await {
                Err(e) => {
                    info!("reconcile error: {:?}", e);
                }
                Ok(_) => {}
            };
            info!("full run complete");
            tokio::time::sleep(Duration::from_secs(args.full_run_interval_seconds)).await;
        }
    });

    let reconcile_task = tokio::spawn(async move {
        let args = reconcile_args;
        // TODO: proper error handling
        let client = Client::try_default().await.unwrap();
        let discovery = Discovery::new(client.clone()).run().await.unwrap();
        let mut last_git_link = tokio::fs::read_link(&args.path).await.unwrap();
        loop {
            // TODO: replace with inotify on the .git file contents
            let current_git_link = tokio::fs::read_link(&args.path).await.unwrap();
            if last_git_link != current_git_link {
                info!("starting quick run");
                match reconcile(&args, &full_path, &client, &discovery).await {
                    Err(e) => {
                        info!("reconcile error: {:?}", e);
                    }
                    Ok(_) => {}
                };
                info!("quick run complete");
            }
            last_git_link = current_git_link;
            // check if the file contents match every second
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // wait for threads to finish
    metrics_task.await?;
    full_run_task.await?;
    reconcile_task.await?;

    Ok(())
}

async fn reconcile(
    args: &Args,
    full_path: &str,
    client: &Client,
    discovery: &Discovery,
) -> Result<()> {
    info!("full path: {}", full_path);
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
        let res =
            kubeclient::apply(client.to_owned(), &discovery, path_str, &args.user_agent).await;
        let elapsed = now.elapsed();
        let success = &res.is_ok().to_string();

        RUN_LATENCY
            .with_label_values(&[success, "QuickRun"])
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
