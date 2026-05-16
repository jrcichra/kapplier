use lazy_static::lazy_static;
use prometheus::{self, register_gauge, register_gauge_vec, Encoder, Gauge, GaugeVec, TextEncoder};

// maintain compatibility with existing kube-applier metrics
lazy_static! {
    // https://github.com/box/kube-applier/blob/5e4f51cc613e7518531363ccf3a853b1e70d012c/metrics/prometheus.go#L27C28-L27C28
    pub static ref FILE_APPLY_COUNT: GaugeVec = register_gauge_vec!(
        "file_apply_count",
        "Success metric for every file applied",
        &["success","file"]
    )
    .unwrap();

    //https://github.com/box/kube-applier/blob/5e4f51cc613e7518531363ccf3a853b1e70d012c/metrics/prometheus.go#L37
   pub static ref RUN_LATENCY: GaugeVec = register_gauge_vec!(
        "run_latency_seconds",
        "Latency for completed apply runs",
        &["success","file"]
    )
    .unwrap();

    pub static ref RECONCILE_DURATION_SECONDS: Gauge = register_gauge!(
        "reconcile_duration_seconds",
        "Total wall-clock time for a full reconcile run"
    )
    .unwrap();

    pub static ref RECONCILE_FAILURE_COUNT: Gauge = register_gauge!(
        "reconcile_failure_count",
        "Number of apply failures in the last reconcile run"
    )
    .unwrap();
}

pub async fn gather_metrics() -> String {
    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    let encoder = TextEncoder::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}
