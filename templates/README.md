# Templates

Copy these into a **consumer** crate. They are not compiled as part of `atomos`.

| File | Kind |
|---|---|
| `atom_pure.rs` | Read-only atom (`signal.get`, `resource.get` style) |
| `atom_effectful.rs` | Write atom (`json.crud`, `server.stop` style) |
| `molecule.rs` | Named list of atom names |
| `module_sync.rs` | `Module` for static / CPU-light paths |
| `module_async.rs` | `AsyncModule` for `/api/*` |
| `module_pre.rs` | Optional pre-ruleset hook |
| `module_post.rs` | Optional post-module hook |
| `config.json` | Kernel JSON |
| `rules.json` | Disjoint include/exclude + optional header rule |
| `error.html` | `{{code}}` `{{phrase}}` `{{detail}}` |

Hot-reload of `.rs` is not supported. Reload **JSON** rules with the control
command `refresh-endpoints` (`rules.reload` atom).

Worked first app: [docs/first-web-app.md](../docs/first-web-app.md) and
`examples/first_app.rs`.
