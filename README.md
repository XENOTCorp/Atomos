# ATOMOS

Extractable HTTP kernel: disjoint JSON rules, modules (`In` → `Out`),
pure/effectful atoms, operator CLI / JSON API. No axum, no regex router, no
reqwest.

Default listen: **127.0.0.1:8090**. This crate does not bind 8082.

## Quick start

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target   # /tmp is noexec on XENOT
cargo run --release --example first_app -- 127.0.0.1:8090
```

Then:

```bash
curl http://127.0.0.1:8090/api/health
curl --http2-prior-knowledge http://127.0.0.1:8090/api/health
curl -k --http2 https://127.0.0.1:8090/api/health
curl -k --http3-only https://127.0.0.1:8090/api/health
```

Walkthrough:
[docs/first-web-app.md](docs/first-web-app.md).

Operator ctl (other terminal, `>` prompt or JSON lines):

```bash
cargo run --release --bin atomos-ctl -- --config examples/first_app/config.json --data examples/data.json
echo '{"cmd":"status"}' | cargo run --release --bin atomos-ctl -- --config examples/first_app/config.json --json
# optional: cargo run --release --bin atomos-ctl -- install-link   # ~/atomos
```

Consumer crate:

```toml
atomos = { path = "../Atomos" }
```

Register a `Module`, parse `Ruleset`, call `atomos::serve::run`.

## Layout

| Path | What |
|---|---|
| `src/kernel/` | `In`/`Out`, rules, cache, governor |
| `src/net/` | listen, engines (`tokio` linked; `epoll`/`xdp` slots) |
| `fds-core` (path dep) | the H1 engine's reactor, TCP transport, and connection table (sibling repo `~/Projects/FDS`) |
| `src/ops/` | atoms, ctl, supervisor |
| `src/plugin/` | manifests; Wasm WIT; `.so` refused |
| `wit/` | `atomos-module.wit` |
| `docs/lack.md` | gap vs h2o/nginx/Pingora |
| `docs/planes.md` | plug layout |
| `src/ctl.rs` | operator CLI / JSON API |
| `src/bin/serve.rs` | `atomos` static server |
| `src/bin/ctl.rs` | `atomos-ctl` |
| `examples/` | `first_app`, `static_site`, `echo_api`, sample config/rules/data |
| `templates/` | copy-paste atoms, molecules, modules, JSON |
| `docs/` | first-app guide, architecture, atoms, modules, ctl, performance, config |

## Guides

1. **[Your first web app](docs/first-web-app.md)** — 3 APIs, rules, hot-swap, pre/post, RAM, cache
2. [Load test (rps, latency, RSS, CPU)](docs/bench-first-app.md)
3. [Architecture](docs/architecture.md)
4. [Atoms and molecules](docs/atoms.md)
5. [Modules and rules](docs/modules.md)
6. [Operator ctl](docs/ctl.md)
7. [Performance](docs/performance.md)
8. [Config](docs/config.md)
9. [Templates](templates/README.md)
10. [Planes / plugins](docs/planes.md)
11. [Gap list](docs/lack.md)
12. [Scorecard](docs/scorecard.md)

Device facts (no hardcoded CPU name or ports in the kernel):

```bash
unset RUSTFLAGS
scripts/atomos-host.sh write
# writes .cargo/config.toml from /proc/cpuinfo
# writes .atomos/host.json (nproc, workers, L3-sized cache, refuse_ports)
```

## Build

lld + RELRO/now/noexecstack via `.cargo/config.toml`. Release: opt-3, thin LTO,
codegen-units 1, panic abort, strip. Warnings are errors on the library.
`cargo clippy --all-targets -- -D warnings`.

Warnings-as-errors and ARCSS (`~/arcss.txt`) apply to this crate the same way
as the original kernel extract.
