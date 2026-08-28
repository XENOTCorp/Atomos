# Plugins

Drop `*.json` manifests in `plugin_dir` (config). Then call `plugin::load_dir`.

| `kind` | Meaning |
|---|---|
| `builtin` | Name must already be inserted on the router |
| `wasm` | Component at `path` implementing `wit/atomos-module.wit` |
| `native` | Refused. `.so` is not a sandbox |

Hot-swap: `Router::insert` is `ArcSwap` for builtin modules. Wasm swap lands when a wasmtime backend is linked. Cache-hit GET never calls a plugin.

Example builtin:

```json
{"name":"static","kind":"builtin"}
```

See [Docs/Wiki/Plugins.md](../../Docs/Wiki/Plugins.md).
