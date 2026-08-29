# Modules

A module has a name and `handle(&In) -> Result<Out, ServeError>`.

Insert the module on the router. Load the ruleset. Start an engine.

```rust
struct Health;
impl Module for Health {
    fn name(&self) -> &'static str { "health" }
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        Ok(Out::json(Status::OK, json_out::to_bytes(&json!({"ok": true}))))
    }
}
```

## Pre and post

Pre and post hooks are global. They are optional.

| Hook | When | Contract |
|---|---|---|
| pre | After parse. Before the ruleset | Status 400 or higher returns. Else flags copy onto the request |
| module | After a ruleset match | `In` borrowed. `Out` owned |
| post | After the module. Unless `FLAG_NO_POST` | Status 0 skips merge. Headers append. Non-empty body replaces. Non-200 status replaces |

Cache-hit GET skips all three.

Config:

```
"pre_module": "pre",
"post_module": "post"
```

The names must exist in the module map. Call `Router::bind_hooks` after insert.

Do not block in `handle`. Do not allocate on the H1 path.

Templates: `Code/templates/module_sync.rs`, `module_async.rs`, `module_pre.rs`, `module_post.rs`.

Charts: [Architecture.md](Architecture.md).
