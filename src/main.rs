use std::{path::Path, time::Duration};

use anyhow::Result;
use clap::Parser;
use futures::{
    channel::mpsc::{channel, Receiver},
    SinkExt, StreamExt,
};
use kube::{Client, Discovery};
use log::{error, info, trace};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use walkdir::WalkDir;
pub mod kubeclient;

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, default_value = "kapplier")]
    user_agent: String,
    #[clap(long, default_value = "content")]
    path: String,
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

    // full run thread
    tokio::spawn(async move {
        let args = full_run_args;
        // TODO: use one client for both threads
        // TODO: proper error handling
        let client = Client::try_default().await.unwrap();
        let discovery = Discovery::new(client.clone()).run().await.unwrap();
        loop {
            info!("starting full run");
            let walker = WalkDir::new(&args.path).into_iter();
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

    // file watch loop
    async_watch(args).await?;

    Ok(())
}

fn async_watcher() -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
    let (mut tx, rx) = channel(1);

    // Automatically select the best implementation for your platform.
    // You can also access each implementation directly e.g. INotifyWatcher.
    let watcher = RecommendedWatcher::new(
        move |res| {
            futures::executor::block_on(async {
                tx.send(res).await.unwrap();
            })
        },
        Config::default(),
    )?;

    Ok((watcher, rx))
}

async fn async_watch(args: Args) -> Result<()> {
    let (mut watcher, mut rx) = async_watcher()?;
    // Add a path to be watched. All files and directories at that path and
    // below will be monitored for changes.
    watcher.watch(args.path.as_ref(), RecursiveMode::Recursive)?;

    let client = Client::try_default().await?;
    let discovery = Discovery::new(client.clone()).run().await?;

    while let Some(res) = rx.next().await {
        match res {
            Ok(event) => {
                reconcile(&args, &event, &client, &discovery).await;
            }
            Err(e) => error!("watch error: {:?}", e),
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

async fn reconcile(args: &Args, event: &Event, client: &Client, discovery: &Discovery) {
    // TODO: make filtering better
    if event.kind.is_access() || event.kind.is_other() {
        trace!("event is intentionally excluded: {:?}", event.kind);
        return;
    }
    for path in &event.paths {
        if should_be_applied(path, args) {
            // trigger a kubectl apply update
            let res = kubeclient::apply(
                client.to_owned(),
                &discovery,
                path.to_str().unwrap(),
                &args.user_agent,
            )
            .await;
            if let Err(x) = res {
                error!("apply error: {}", x);
                return;
            }
        }
    }
}
