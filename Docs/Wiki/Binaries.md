# Binaries

| Name | Path | Role |
|---|---|---|
| `atomos` | `Code/src/bin/serve.rs` | HTTP/1.1 origin on epoll. Loopback by default. |
| `atomos-proto` | `Code/src/bin/proto.rs` | HTTP/2, HTTP/3, TLS on tokio. |
| `atomos-ctl` | `Code/src/bin/ctl.rs` | Operator CLI and JSON API. Separate process. |
| `atomos-sup` | `Code/src/bin/sup.rs` | Spawns pinned workers. Does not bind HTTP. |
| `atomos-keyd` | `Code/src/bin/keyd.rs` | Private keys out of workers. Unix sign daemon. |

Default bind is `127.0.0.1:8090`.
