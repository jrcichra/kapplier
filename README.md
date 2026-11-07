# kapplier

A simpler (and faster) alternative to [kube-applier](https://github.com/box/kube-applier).

Applies the YAML files in a directory with server-side apply: on start, on a fixed interval, and when a webhook is called. No GUI, no `git`, no `kubectl` binary required.

Intended to be used with a [git-sync](https://github.com/kubernetes/git-sync) sidecar (>=v4), which syncs a git repo to disk and calls the webhook after each sync.

## Endpoints

| Endpoint | Method | Description |
|---|---|---|
| `/webhook` | POST | Trigger an immediate reconcile run |
| `/metrics` | GET | Prometheus metrics |

### Metrics

| Metric | Labels | Description |
|---|---|---|
| `file_apply_count` | `success`, `file` | Number of times each file has been applied (counter) |
| `run_latency_seconds` | `success`, `file` | Time taken to apply each file |
| `reconcile_duration_seconds` | — | Total wall-clock time for the last reconcile run |
| `reconcile_failure_count` | — | Number of apply failures in the last reconcile run |
| `last_run_timestamp_seconds` | — | Unix time the last reconcile run finished |

Alert on `reconcile_failure_count > 0` (an apply has been failing) and on
`time() - last_run_timestamp_seconds` growing past a few intervals (runs have
stopped).

## Behaviour

- Runs one reconcile at a time: on start, every `--full-run-interval-seconds`,
  and after a webhook call. Webhooks during a run collapse into one more run.
- Applies with server-side apply (`--user-agent` is the field manager).
- If a document's kind is unknown (say, its CRD is applied earlier in the same
  run), the file is retried once with fresh API discovery.
- On SIGTERM it finishes the current run, then exits.
- Doesn't prune: deleting a manifest leaves the object in the cluster.

## Configuration

All options can be set as CLI flags or environment variables.

```
Usage: kapplier [OPTIONS]

Options:
      --user-agent <USER_AGENT>
          [env: KAPPLIER_USER_AGENT] [default: kapplier]
      --path <PATH>
          [env: KAPPLIER_PATH] [default: repo]
      --subpath <SUBPATH>
          [env: KAPPLIER_SUBPATH] [default: ]
      --ignore-hidden-directories <IGNORE_HIDDEN_DIRECTORIES>
          [env: KAPPLIER_IGNORE_HIDDEN_DIRECTORIES] [default: true] [possible values: true, false]
      --supported-extensions <SUPPORTED_EXTENSIONS>
          [env: KAPPLIER_SUPPORTED_EXTENSIONS] [default: yml yaml]
      --full-run-interval-seconds <FULL_RUN_INTERVAL_SECONDS>
          [env: KAPPLIER_FULL_RUN_INTERVAL_SECONDS] [default: 300]
      --webserver-port <WEBSERVER_PORT>
          [env: KAPPLIER_WEBSERVER_PORT] [default: 9100]
      --filter-annotation <FILTER_ANNOTATION>
          [env: KAPPLIER_FILTER_ANNOTATION] Only apply documents with this annotation (e.g. kapplier.io/managed=true or just kapplier.io/managed)
      --filter-label <FILTER_LABEL>
          [env: KAPPLIER_FILTER_LABEL] Only apply documents with this label (e.g. app=myapp or just app)
  -h, --help
          Print help
  -V, --version
          Print version
```

## Example

See [examples/kapplier.yaml](./examples/kapplier.yaml) for a full Kubernetes deployment with a git-sync sidecar.

The example configures git-sync to sync a repo to `/repo` and call the webhook after each sync, while kapplier watches `/repo/kapplier.git/deploy` for YAML files to apply.

## Development

```sh
cargo clippy --all-targets && cargo test
```

`src/main.rs` has the CLI, web server and run loop; `src/reconcile.rs` one
run over the directory; `src/apply.rs` one file's documents; `src/metrics.rs`
the Prometheus metrics. CI builds native amd64 and arm64 binaries and
publishes `ghcr.io/jrcichra/kapplier` with `latest` and `sha-<8-char commit>`
tags.

