# Modules

A module has a name and `handle(&In) -> Result<Out, ServeError>`.

Pre and post hooks are global. Pre runs before the ruleset. Pre may short-circuit with status 400 or higher. Post sees the flags and may rewrite the response. Both are optional. Set them in config.

```rust
struct Health;
impl Module for Health {
    fn name(&self) -> &'static str { "health" }
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        Ok(Out::json(Status::OK, json_out::to_bytes(&json!({"ok": true}))))
    }
}
```

Insert the module on the router. Load the ruleset. Start an engine.

Templates: `Code/templates/module_sync.rs`, `module_async.rs`, `module_pre.rs`, `module_post.rs`.
