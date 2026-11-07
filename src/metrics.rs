//! Prometheus metrics on `/metrics`. Names are kept compatible with
//! kube-applier:
//! <https://github.com/box/kube-applier/blob/5e4f51cc613e7518531363ccf3a853b1e70d012c/metrics/prometheus.go>

use prometheus::{
    CounterVec, Gauge, GaugeVec, TextEncoder, register_counter_vec, register_gauge,
    register_gauge_vec,
};
use std::sync::LazyLock;

pub static FILE_APPLY_COUNT: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "file_apply_count",
        "Number of times each file was applied, by outcome",
        &["success", "file"]
    )
    .expect("register file_apply_count")
});

pub static RUN_LATENCY: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "run_latency_seconds",
        "Time the last apply of each file took (one series per file, labelled with its outcome)",
        &["success", "file"]
    )
    .expect("register run_latency_seconds")
});

pub static RECONCILE_DURATION_SECONDS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "reconcile_duration_seconds",
        "Total wall-clock time for the last reconcile run"
    )
    .expect("register reconcile_duration_seconds")
});

pub static RECONCILE_FAILURE_COUNT: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "reconcile_failure_count",
        "Number of apply failures in the last reconcile run"
    )
    .expect("register reconcile_failure_count")
});

pub static LAST_RUN_TIMESTAMP_SECONDS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "last_run_timestamp_seconds",
        "Unix time the last reconcile run finished (with or without failures)"
    )
    .expect("register last_run_timestamp_seconds")
});

/// Axum handler for `/metrics`.
pub async fn gather() -> String {
    TextEncoder::new()
        .encode_to_string(&prometheus::gather())
        .unwrap_or_else(|e| format!("# failed to encode metrics: {e}\n"))
}
