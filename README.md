# kapplier

A simpler (and faster) alternative to [kube-applier](https://github.com/box/kube-applier).

No GUI, no `git`, no `kubectl`. Only requires a directory to watch.

Expected to be used with a [git-sync](https://github.com/kubernetes/git-sync) sidecar.

```
Usage: kapplier [OPTIONS]

Options:
      --user-agent <USER_AGENT>                                [default: kapplier]
      --path <PATH>                                            [default: content]
      --ignore-hidden-directories
      --supported-extensions <SUPPORTED_EXTENSIONS>            [default: yml yaml]
      --full-run-interval-seconds <FULL_RUN_INTERVAL_SECONDS>  [default: 300]
  -h, --help                                                   Print help
  -V, --version                                                Print version
```
