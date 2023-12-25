# kapplier

A simpler (and faster) alternative to [kube-applier](https://github.com/box/kube-applier).

No GUI, no `git`, no `kubectl`. Only requires a directory to watch.

Expected to be used with a [git-sync](https://github.com/kubernetes/git-sync) sidecar >=v4.

```
Usage: kapplier [OPTIONS]

Options:
      --user-agent <USER_AGENT>                                [default: kapplier]
      --path <PATH>                                            [default: repo]
      --subpath <SUBPATH>                                      [default: ]
      --ignore-hidden-directories
      --supported-extensions <SUPPORTED_EXTENSIONS>            [default: yml yaml]
      --full-run-interval-seconds <FULL_RUN_INTERVAL_SECONDS>  [default: 300]
      --metrics-port <METRICS_PORT>                            [default: 9100]
  -h, --help                                                   Print help
  -V, --version                                                Print version
```
