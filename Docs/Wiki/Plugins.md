# Plugins

Four planes include a plugin surface. Plugins declare a JSON manifest in `plugin_dir`. Call `plugin::load_dir`.

| `kind` | Meaning |
|---|---|
| `builtin` | Name must already be inserted on the router |
| `wasm` | Component at `path` implementing `Code/wit/atomos-module.wit` |
| `native` | Refused. `.so` is not a sandbox |

Hot-swap: `Router::insert` is `ArcSwap` for named modules. `plugin::reload` reads `plugin_dir` again. Cache-hit GET never calls a plugin. Native `.so` is refused.

See [Architecture.md](Architecture.md).

Example builtin:

```json
{"name":"static","kind":"builtin"}
```

See `Code/plugins/example/static.json`.
