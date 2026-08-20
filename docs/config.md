# Config JSON

`Config::from_json` / `Config::load_path`. Missing `bind` defaults to
`127.0.0.1:8090`. Non-loopback bind is an error unless
`allow_non_loopback: true`.

| Field | Role |
|---|---|
| `bind` | `host:port` |
| `workers` | tokio worker hint (min 1) |
| `tcp_nodelay` / `so_reuseport` / `tcp_fastopen` | listen opts |
| `max_header_bytes` / `max_body_bytes` / `max_json_depth` | parse caps |
| `memory_cap_bytes` / `memory_mode` | `hard` or `degrade` |
| `cache_entries` / `cache_bytes` | response cache |
| `pre_module` / `post_module` | optional names (you still register the `Module`) |
| `rules_path` | JSON ruleset, hot-reloadable |
| `control_socket` | Unix socket, mode 0600 |
| `static_root` / `error_page` | files |

See `templates/config.json` and `examples/config.json`.
