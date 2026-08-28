# Plugins

Drop `*.json` manifests in `plugin_dir` (config). Then `plugin::load_dir`.

| `kind` | Meaning |
|---|---|
| `builtin` | Name must already be `Router::insert`ed in-process |
| `wasm` | Component at `path` implementing `wit/atomos-module.wit` (host not linked yet) |
| `native` | **Refused.** `.so` is not a sandbox |

Hot-swap: `Router::insert` is `ArcSwap` (native). Wasm swap lands when a wasmtime backend is linked. Cache-hit GET never calls a plugin.

Example builtin:

```json
{"name":"static","kind":"builtin"}
```
