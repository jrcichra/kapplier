//! Applies the Kubernetes YAML in a directory (kept in sync by git-sync) with
//! server-side apply: on start, on an interval, and when git-sync calls the
//! webhook.

mod apply;
mod metrics;
mod reconcile;

use anyhow::Result;
use axum::{
    Router,
    extract::State,
    routing::{get, post},
};
use clap::Parser;
use kube::Client;
use std::future::Future;
use std::io::IsTerminal;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

#[derive(Parser, Debug, Clone)]
#[command(version, about)]
pub struct Args {
    /// Field manager for server-side apply.
    #[arg(long, env = "KAPPLIER_USER_AGENT", default_value = "kapplier")]
    pub user_agent: String,
    #[arg(long, env = "KAPPLIER_PATH", default_value = "repo")]
    pub path: String,
    #[arg(long, env = "KAPPLIER_SUBPATH", default_value = "")]
    pub subpath: String,
    #[arg(long, env = "KAPPLIER_IGNORE_HIDDEN_DIRECTORIES", action = clap::ArgAction::Set, default_value = "true")]
    pub ignore_hidden_directories: bool,
    #[arg(long, env = "KAPPLIER_SUPPORTED_EXTENSIONS", default_values = ["yml", "yaml"])]
    pub supported_extensions: Vec<String>,
    #[arg(
        long,
        env = "KAPPLIER_FULL_RUN_INTERVAL_SECONDS",
        default_value_t = 300
    )]
    pub full_run_interval_seconds: u64,
    #[arg(long, env = "KAPPLIER_WEBSERVER_PORT", default_value_t = 9100)]
    pub webserver_port: u16,
    /// Only apply documents that have this annotation. Format: key=value or just key to check presence.
    #[arg(long, env = "KAPPLIER_FILTER_ANNOTATION")]
    pub filter_annotation: Option<String>,
    /// Only apply documents that have this label. Format: key=value or just key to check presence.
    #[arg(long, env = "KAPPLIER_FILTER_LABEL")]
    pub filter_label: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    let args = Args::parse();
    let full_path = if args.subpath.is_empty() {
        args.path.clone()
    } else {
        format!("{}/{}", args.path, args.subpath)
    };
    let client = Client::try_default().await?;

    let trigger = Arc::new(Notify::new());
    let app = Router::new()
        .route("/metrics", get(metrics::gather))
        .route("/webhook", post(webhook))
        .with_state(trigger.clone());
    let listener = TcpListener::bind(("0.0.0.0", args.webserver_port)).await?;
    info!("listening on :{}", args.webserver_port);
    let mut server = tokio::spawn(async move { axum::serve(listener, app).await });

    // first tick fires immediately, so a restarted kapplier applies right away
    let mut ticker = interval(Duration::from_secs(args.full_run_interval_seconds));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // installed up front, so a SIGTERM during a run is acted on once the run
    // finishes (a run is never cut off half-applied)
    let mut shutdown = pin!(shutdown_signal()?);

    loop {
        tokio::select! {
            () = &mut shutdown => break,
            res = &mut server => anyhow::bail!("web server stopped: {res:?}"),
            _ = ticker.tick() => info!("starting full run"),
            () = trigger.notified() => info!("starting webhook run"),
        }
        if let Err(e) = reconcile::run(&args, &full_path, &client).await {
            warn!("reconcile failed: {e:#}");
        }
    }
    info!("shutting down");
    Ok(())
}

/// The webhook only asks for a run. Runs happen one at a time in `main`'s
/// loop, and any number of webhooks during a run collapse into one more run.
async fn webhook(State(trigger): State<Arc<Notify>>) -> &'static str {
    debug!("webhook: run requested");
    trigger.notify_one();
    "ok"
}

/// Resolves on SIGTERM or SIGINT.
fn shutdown_signal() -> Result<impl Future<Output = ()>> {
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = term.recv() => info!("received SIGTERM"),
            _ = int.recv() => info!("received SIGINT"),
        }
    })
}
