# kapplier

A simpler (and faster) alternative to [kube-applier](https://github.com/box/kube-applier).

Watches a directory for YAML files and runs `kubectl apply` on them — on a fixed interval and/or triggered by a webhook. No GUI, no `git`, no `kubectl` binary required.

Intended to be used with a [git-sync](https://github.com/kubernetes/git-sync) sidecar (>=v4), which syncs a git repo to disk and calls the webhook after each sync.

## Endpoints

| Endpoint | Method | Description |
|---|---|---|
| `/webhook` | POST | Trigger an immediate reconcile run |
| `/metrics` | GET | Prometheus metrics |

### Metrics

| Metric | Labels | Description |
|---|---|---|
| `file_apply_count` | `success`, `file` | Number of times each file has been applied |
| `run_latency_seconds` | `success`, `file` | Time taken to apply each file |
| `reconcile_duration_seconds` | — | Total wall-clock time for the last reconcile run |
| `reconcile_failure_count` | — | Number of apply failures in the last reconcile run |

## Configuration

All options can be set as CLI flags or environment variables.

```
Usage: kapplier [OPTIONS]

Options:
      --user-agent <USER_AGENT>
          [env: USER_AGENT] [default: kapplier]
      --path <PATH>
          [env: PATH] [default: repo]
      --subpath <SUBPATH>
          [env: SUBPATH] [default: ]
      --ignore-hidden-directories <IGNORE_HIDDEN_DIRECTORIES>
          [env: IGNORE_HIDDEN_DIRECTORIES] [default: true] [possible values: true, false]
      --supported-extensions <SUPPORTED_EXTENSIONS>
          [env: SUPPORTED_EXTENSIONS] [default: yml yaml]
      --full-run-interval-seconds <FULL_RUN_INTERVAL_SECONDS>
          [env: FULL_RUN_INTERVAL_SECONDS] [default: 300]
      --webserver-port <WEBSERVER_PORT>
          [env: WEBSERVER_PORT] [default: 9100]
      --filter-annotation <FILTER_ANNOTATION>
          [env: FILTER_ANNOTATION] Only apply documents with this annotation (e.g. kapplier.io/managed=true or just kapplier.io/managed)
  -h, --help
          Print help
  -V, --version
          Print version
```

## Example

See [examples/kapplier.yaml](./examples/kapplier.yaml) for a full Kubernetes deployment with a git-sync sidecar.

The example configures git-sync to sync a repo to `/repo` and call the webhook after each sync, while kapplier watches `/repo/kapplier.git/deploy` for YAML files to apply.
