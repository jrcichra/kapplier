use anyhow::{Context, Result};
use clap::Parser;
use kube::{Client, Discovery};
use log::{info, trace};
use std::{path::Path, process, thread, time::Duration};
use walkdir::WalkDir;
pub mod kubeclient;

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, default_value = "kapplier")]
    user_agent: String,
    #[clap(long, default_value = "repo")]
    path: String,
    #[clap(long, default_value = "dist")]
    subpath: String,
    #[clap(long, default_value = "true")]
    ignore_hidden_directories: bool,
    #[clap(long, default_values = ["yml", "yaml"])]
    supported_extensions: Vec<String>,
    #[clap(long, default_value = "300")]
    full_run_interval_seconds: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // setup
    simple_logger::init_with_level(log::Level::Info)?;
    let args = Args::parse();
    let full_run_args = args.clone();
    let reconcile_args: Args = args.clone();
    let full_path = format!("{}/{}", &args.path, &args.subpath);

    // handle control c
    ctrlc::set_handler(move || {
        info!("received Ctrl+C! Exiting...");
        // exit immediately
        process::exit(0);
    })?;

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
            let walker = WalkDir::new(&full_path_clone).into_iter();
            // go through all files in the path
            for entry in walker {
                let entry = entry.unwrap();
                let path = entry.path();
                if !should_be_applied(&path, &args) {
                    continue;
                }
                kubeclient::apply(
                    client.to_owned(),
                    &discovery,
                    path.to_str().unwrap(),
                    &args.user_agent,
                )
                .await
                .unwrap();
            }
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
            let current_git_link = tokio::fs::read_link(&args.path).await.unwrap();
            if last_git_link != current_git_link {
                match reconcile(&args, &full_path, &client, &discovery).await {
                    Err(e) => {
                        info!("reconcile error: {:?}", e);
                    }
                    Ok(_) => {}
                };
            }
            last_git_link = current_git_link;
            // check if the file contents match every second
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // wait for threads to finish
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
    let walker = WalkDir::new(full_path).into_iter();
    for path in walker {
        let path = path.context("could not unwrap path")?;
        if should_be_applied(path.path(), args) {
            // trigger a kubectl apply update
            kubeclient::apply(
                client.to_owned(),
                &discovery,
                path.path()
                    .to_str()
                    .context("could not convert path to str")?,
                &args.user_agent,
            )
            .await?;
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
