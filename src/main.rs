use anyhow::{Context, Result};
use axum::{
    extract::State,
    routing::{get, post},
    Router,
};
use clap::Parser;
use kube::Client;
use log::{debug, info, trace, warn};
use prometheus::{
    FILE_APPLY_COUNT, LAST_RUN_TIMESTAMP_SECONDS, RECONCILE_DURATION_SECONDS,
    RECONCILE_FAILURE_COUNT, RUN_LATENCY,
};
use std::{
    collections::BTreeMap,
    path::Path,
    pin::pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpListener,
    signal::unix::{signal, SignalKind},
    sync::Notify,
    time::{interval, Instant, MissedTickBehavior},
};
use walkdir::WalkDir;
pub mod kubeclient;
pub mod prometheus;

#[derive(Parser, Debug, Clone)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, env = "KAPPLIER_USER_AGENT", default_value = "kapplier")]
    user_agent: String,
    #[clap(long, env = "KAPPLIER_PATH", default_value = "repo")]
    path: String,
    #[clap(long, env = "KAPPLIER_SUBPATH", default_value = "")]
    subpath: String,
    #[clap(long, env = "KAPPLIER_IGNORE_HIDDEN_DIRECTORIES", action = clap::ArgAction::Set, default_value = "true")]
    ignore_hidden_directories: bool,
    #[clap(long, env = "KAPPLIER_SUPPORTED_EXTENSIONS", default_values = ["yml", "yaml"])]
    supported_extensions: Vec<String>,
    #[clap(
        long,
        env = "KAPPLIER_FULL_RUN_INTERVAL_SECONDS",
        default_value = "300"
    )]
    full_run_interval_seconds: u64,
    #[clap(long, env = "KAPPLIER_WEBSERVER_PORT", default_value = "9100")]
    webserver_port: u16,
    /// Only apply documents that have this annotation. Format: key=value or just key to check presence.
    #[clap(long, env = "KAPPLIER_FILTER_ANNOTATION")]
    filter_annotation: Option<String>,
    /// Only apply documents that have this label. Format: key=value or just key to check presence.
    #[clap(long, env = "KAPPLIER_FILTER_LABEL")]
    filter_label: Option<String>,
}

/// The webhook only asks for a run. Runs happen one at a time in `main`'s
/// loop, and any number of webhooks during a run collapse into one more run.
async fn webhook(State(trigger): State<Arc<Notify>>) -> &'static str {
    debug!("webhook: run requested");
    trigger.notify_one();
    "ok"
}

#[tokio::main]
async fn main() -> Result<()> {
    simple_logger::init_with_level(log::Level::Info)?;
    let args = Args::parse();
    let full_path = if args.subpath.is_empty() {
        args.path.clone()
    } else {
        format!("{}/{}", args.path, args.subpath)
    };
    let client = Client::try_default().await?;

    let trigger = Arc::new(Notify::new());
    let app = Router::new()
        .route("/metrics", get(prometheus::gather_metrics))
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
            res = &mut server => anyhow::bail!("web server stopped: {:?}", res),
            _ = ticker.tick() => info!("starting full run"),
            () = trigger.notified() => info!("starting webhook run"),
        }
        if let Err(e) = reconcile(&args, &full_path, &client).await {
            warn!("reconcile failed: {e:#}");
        }
    }
    info!("shutting down");
    Ok(())
}

/// Resolves on SIGTERM or SIGINT.
fn shutdown_signal() -> Result<impl std::future::Future<Output = ()>> {
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = term.recv() => info!("received SIGTERM"),
            _ = int.recv() => info!("received SIGINT"),
        }
    })
}

async fn reconcile(args: &Args, full_path: &str, client: &Client) -> Result<()> {
    let start = Instant::now();
    let mut discovery = kubeclient::run_discovery(client.clone()).await?;
    let filter = args.filter_annotation.as_deref();
    let filter_label = args.filter_label.as_deref();

    let mut files = vec![];
    for entry in WalkDir::new(full_path).sort_by_file_name() {
        let entry = entry.context("could not unwrap entry")?;
        let path = entry.path();
        if should_be_applied(path, args) {
            files.push(
                path.to_str()
                    .context("could not convert path to str")?
                    .to_owned(),
            );
        }
    }

    let mut results = BTreeMap::new();
    for path in &files {
        let now = Instant::now();
        let res = kubeclient::apply(
            client,
            &discovery,
            path,
            &args.user_agent,
            filter,
            filter_label,
        )
        .await;
        results.insert(path.clone(), (res, now.elapsed()));
    }

    // a CRD and its first objects can land in the same run: discovery ran
    // before the CRD existed, so retry those files once with fresh discovery
    let retry: Vec<_> = results
        .iter()
        .filter(|(_, (res, _))| matches!(res, Ok(o) if o.unknown_kinds > 0))
        .map(|(path, _)| path.clone())
        .collect();
    if !retry.is_empty() {
        info!(
            "{} files had unknown kinds; refreshing discovery and retrying them",
            retry.len()
        );
        discovery = kubeclient::run_discovery(client.clone()).await?;
        for path in retry {
            let now = Instant::now();
            let res = kubeclient::apply(
                client,
                &discovery,
                &path,
                &args.user_agent,
                filter,
                filter_label,
            )
            .await;
            results.insert(path, (res, now.elapsed()));
        }
    }

    let mut total_failures: i64 = 0;
    for (path, (res, elapsed)) in &results {
        let ok = match res {
            Ok(o) => {
                total_failures += o.failures;
                o.failures == 0
            }
            Err(e) => {
                warn!("failed to apply {path}: {e:#}");
                total_failures += 1;
                false
            }
        };
        let (success, stale) = if ok {
            ("true", "false")
        } else {
            ("false", "true")
        };
        // one latency series per file: drop the other outcome's stale value
        let _ = RUN_LATENCY.remove_label_values(&[stale, path]);
        RUN_LATENCY
            .with_label_values(&[success, path])
            .set(elapsed.as_secs_f64());
        FILE_APPLY_COUNT.with_label_values(&[success, path]).inc();
    }

    let elapsed = start.elapsed();
    RECONCILE_DURATION_SECONDS.set(elapsed.as_secs_f64());
    RECONCILE_FAILURE_COUNT.set(total_failures as f64);
    LAST_RUN_TIMESTAMP_SECONDS.set(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64());
    let summary = format!(
        "reconcile complete: {} files in {:.2}s, {} failures",
        results.len(),
        elapsed.as_secs_f64(),
        total_failures
    );
    if total_failures > 0 {
        warn!("{summary}");
    } else {
        info!("{summary}");
    }

    Ok(())
}

fn should_be_applied(path: &Path, args: &Args) -> bool {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return false,
    };

    let hidden = |c: std::path::Component| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.starts_with('.') && s.len() > 1)
    };
    if args.ignore_hidden_directories && path.components().any(hidden) {
        trace!("path is within hidden directory: {}", path_str);
        return false;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_only_visible_yaml() {
        let args = Args::parse_from(["kapplier"]);
        let applied = |p: &str| should_be_applied(Path::new(p), &args);
        assert!(applied("repo/dist/app.yaml"));
        assert!(applied("repo/dist/app.yml"));
        assert!(applied("./repo/app.yaml")); // "." is not a hidden dir
        assert!(!applied("repo/.git/config.yaml"));
        assert!(!applied("repo/dist/README.md"));
        assert!(!applied("repo/dist/Makefile"));
    }
}
