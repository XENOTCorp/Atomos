# Configuration

`config.json` holds the runtime configuration.

Important fields:

| Field | Role |
|---|---|
| `bind` | Listen address. Default `127.0.0.1:8090`. |
| `engine` | `epoll` or `tokio`. |
| `workers` | Pinned worker count. Host overlay may set this. |
| `static_root` | Static files. |
| `rules_path` | Rules JSON. |
| `error_page` | Error HTML. |
| `control_socket` | Unix control socket. |
| `memory_cap_bytes` | RSS cap. |
| `memory_mode` | `hard` or `degrade`. |
| `cache_entries` | Response cache entry cap. Default 4096. |
| `cache_bytes` | Response cache byte cap. Default 16 MiB. Host overlay sets L3. |
| `max_header_bytes` | Header size bound. |
| `max_body_bytes` | Body size bound. |
| `max_json_depth` | JSON depth bound. |
| `header_timeout_ms` | Incomplete headers. Default 10 s. |
| `body_timeout_ms` | Incomplete body. Default 30 s. |
| `idle_timeout_ms` | Keep-alive idle. Default 75 s. |
| `module_timeout_ms` | 504 if `handle` runs longer. Default 5 s. |
| `http2` / `http3` | Proto process only. |
| `tls_cert` / `tls_key` | TLS on proto. H1 epoll TLS is not in this tree yet. |
| `plugin_dir` | Plugin manifests. |
| `pre_module` / `post_module` | Optional hooks. |
| `landlock` / `seccomp` | Post-bind jail. Linux. |

Host facts come from `.atomos/host.json`. `./compile.sh` writes that file. Workers follow `nproc`. `cache_bytes` follows L3 size. `refuse_ports` follows `ATOMOS_REFUSE_PORTS`.

Do not copy `.atomos/host.json` to another machine. See [Compile.md](Compile.md).

Example hard bounds: RSS cap 64 MiB in examples, JSON depth 32, body 262144 bytes, response cache 4096 entries or 16 MiB.
