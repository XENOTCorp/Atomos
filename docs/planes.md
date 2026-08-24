# Planes (plug layout)

Atomos is four planes. Swap a plane; do not grow a blob `src/*.rs`.

```
kernel/   In Out Module Ruleset cache governor   — ABI, never sockets
net/      listen parse encode engines            — EngineKind: tokio | epoll | xdp
ops/      atoms ctl sup                          — not on GET
plugin/   manifests, load_dir                    — builtin | wasm | native-refused
```

Hot path: `net` cache hit → `write`. No plugin, no atom, no Wasm.

Hot-swap:

| What | How |
|---|---|
| Rules | `arc-swap` (`rules.reload`) |
| Native module | `Router::insert` (ArcSwap map) |
| Wasm module | `plugin_dir` + `wit/atomos-module.wit` (host not linked) |
| `.so` | refused |
| Workers | `atomos-sup` restarts children |

I/O: `epoll::run` (H1, default). `serve::run` / `atomos-proto` (H2/H3). `xdp` unlinked.

Supervisor: `atomos-sup [N] [worker-exe] [args…]` sets `ATOMOS_WORKER_INDEX`.
