//! One reconcile run: apply every file under the path, retry files whose kinds
//! were unknown, record metrics.

use crate::{Args, apply, metrics};
use anyhow::{Context, Result};
use kube::{Client, Discovery};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::Instant;
use tracing::{info, warn};
use walkdir::WalkDir;

pub async fn run(args: &Args, full_path: &str, client: &Client) -> Result<()> {
    // the run on start usually beats git-sync's first clone; its webhook
    // starts the real first run
    if !Path::new(full_path).exists() {
        info!("{full_path} doesn't exist yet (git-sync hasn't synced); skipping");
        return Ok(());
    }
    let start = Instant::now();
    let mut files = vec![];
    for entry in WalkDir::new(full_path).sort_by_file_name() {
        let entry = entry.context("could not read directory entry")?;
        if should_apply(entry.path(), args) {
            files.push(entry.path().to_str().context("non-UTF-8 path")?.to_owned());
        }
    }

    let discovery = Discovery::new(client.clone()).run().await?;
    let mut results = BTreeMap::new();
    for path in &files {
        let t = Instant::now();
        let res = apply::file(client, &discovery, path, args).await;
        results.insert(path.clone(), (res, t.elapsed()));
    }

    // a CRD and its first objects can land in the same run: discovery ran
    // before the CRD existed, so retry those files once with fresh discovery
    let retry: Vec<String> = results
        .iter()
        .filter(|(_, (res, _))| matches!(res, Ok(o) if o.unknown_kinds > 0))
        .map(|(path, _)| path.clone())
        .collect();
    if !retry.is_empty() {
        info!(
            "{} files had unknown kinds; refreshing discovery and retrying them",
            retry.len()
        );
        let discovery = Discovery::new(client.clone()).run().await?;
        for path in retry {
            let t = Instant::now();
            let res = apply::file(client, &discovery, &path, args).await;
            results.insert(path, (res, t.elapsed()));
        }
    }

    let mut failures: i64 = 0;
    for (path, (res, elapsed)) in &results {
        let ok = match res {
            Ok(o) => {
                failures += o.failures;
                o.failures == 0
            }
            Err(e) => {
                warn!("{path}: {e:#}");
                failures += 1;
                false
            }
        };
        let (success, stale) = if ok {
            ("true", "false")
        } else {
            ("false", "true")
        };
        // one latency series per file: drop the other outcome's stale value
        let _ = metrics::RUN_LATENCY.remove_label_values(&[stale, path]);
        metrics::RUN_LATENCY
            .with_label_values(&[success, path])
            .set(elapsed.as_secs_f64());
        metrics::FILE_APPLY_COUNT
            .with_label_values(&[success, path])
            .inc();
    }

    let elapsed = start.elapsed().as_secs_f64();
    metrics::RECONCILE_DURATION_SECONDS.set(elapsed);
    metrics::RECONCILE_FAILURE_COUNT.set(failures as f64);
    metrics::LAST_RUN_TIMESTAMP_SECONDS
        .set(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64());
    let summary = format!(
        "reconcile complete: {} files in {elapsed:.2}s, {failures} failures",
        results.len()
    );
    if failures > 0 {
        warn!("{summary}");
    } else {
        info!("{summary}");
    }
    Ok(())
}

/// A file with a supported extension, outside hidden directories (if
/// ignoring them).
fn should_apply(path: &Path, args: &Args) -> bool {
    let hidden = |c: std::path::Component| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.starts_with('.') && s.len() > 1)
    };
    if args.ignore_hidden_directories && path.components().any(hidden) {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| args.supported_extensions.iter().any(|s| s == e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn applies_only_visible_yaml() {
        let args = Args::parse_from(["kapplier"]);
        let applied = |p: &str| should_apply(Path::new(p), &args);
        assert!(applied("repo/dist/app.yaml"));
        assert!(applied("repo/dist/app.yml"));
        assert!(applied("./repo/app.yaml")); // "." is not a hidden dir
        assert!(!applied("repo/.git/config.yaml"));
        assert!(!applied("repo/dist/README.md"));
        assert!(!applied("repo/dist/Makefile"));
    }
}
