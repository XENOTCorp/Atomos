# Config JSON

`Config::from_json` / `Config::load_path`. Missing `bind` defaults to
`127.0.0.1:8090`. Non-loopback bind is an error unless
`allow_non_loopback: true`.

| Field | Role |
|---|---|
| `bind` | `host:port` |
| `workers` | pinned OS threads (default = logical CPUs, min 1) |
| `cpu_pin` | `sched_setaffinity` per worker (default true) |
| `http2` | h2c + TLS ALPN `h2` (default **false**; proto process only) |
| `http3` | QUIC/UDP HTTP/3 (default **false**; proto process only) |
| `tls_cert` / `tls_key` | PEM pair; omitted → ephemeral self-signed for loopback |
| `tcp_nodelay` / `so_reuseport` / `tcp_fastopen` | listen opts |
| `max_header_bytes` / `max_body_bytes` / `max_json_depth` | parse caps |
| `memory_cap_bytes` / `memory_mode` | `hard` or `degrade` |
| `cache_entries` / `cache_bytes` | response cache |
| `pre_module` / `post_module` | names; `Router::bind_hooks` copies registered sync modules |
| `refuse_ports` | ports the process will not bind (from config or host.json) |
| `plugin_dir` | directory of plugin JSON manifests |
| `engine` | default **`epoll`**. `tokio` = `atomos-proto`. `epoll` ∧ (`http2`∨`http3`) is a config error. `xdp` not linked. |
| `worker_shutdown_timeout_ms` | `atomos-sup` SIGTERM drain (default 2000) |
| `landlock` / `seccomp` / `drop_caps` | jail after bind (seccomp/landlock default false) |
| `rules_path` | JSON ruleset, hot-reloadable |
| `control_socket` | Unix socket, mode 0600 |
| `static_root` / `error_page` | files |

Host overlay (`ATOMOS_HOST` or `.atomos/host.json`): workers, cpu_pin,
cache size, refuse_ports. Generate with `scripts/atomos-host.sh write`.
CPU rustc flags: `scripts/cpu-rustflags.sh write`.

See `templates/config.json` and `examples/config.json`.
