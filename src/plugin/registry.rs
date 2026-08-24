//! Scan a directory of `*.json` manifests. Hot-swap is `reload` (ArcSwap).

use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::error::ServeError;
#[cfg(feature = "wasm")]
use crate::module::Handler;
use crate::plugin::manifest::{PluginKind, PluginManifest};
use crate::route::Router;

/// Load manifests from `dir`. Builtin names must already be registered.
/// Native `.so` is refused. Wasm loads when compiled with `feature = "wasm"`.
pub fn load_dir(router: &Router, dir: &Path) -> Result<Vec<String>, ServeError> {
    if !dir.is_dir() {
        return Err(ServeError::Config("plugin dir missing".into()));
    }
    let mut loaded = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read(&p)?;
        let man: PluginManifest = serde_json::from_slice(&raw)
            .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?;
        match man.kind {
            PluginKind::Builtin => {
                if router.module(&man.name).is_none() {
                    return Err(ServeError::Module(man.name.clone().into()));
                }
                loaded.push(man.name);
            }
            PluginKind::Wasm => {
                #[cfg(not(feature = "wasm"))]
                {
                    return Err(ServeError::Config(
                        format!(
                            "wasm plugin {} ({}): host not linked; compile with a wasmtime backend",
                            man.name,
                            man.path.as_deref().unwrap_or("?")
                        )
                        .into(),
                    ));
                }
                #[cfg(feature = "wasm")]
                {
                    let rel = man.path.as_deref().ok_or_else(|| {
                        ServeError::Config(format!("wasm plugin {} missing path", man.name).into())
                    })?;
                    let base = p.parent().unwrap_or(dir);
                    let wasm_path = base.join(rel);
                    let m = crate::plugin::wasm::load(&wasm_path, router.cfg.wasm_fuel)?;
                    router.insert(man.name.clone(), Handler::Sync(m));
                    loaded.push(man.name);
                }
            }
            PluginKind::Native => {
                return Err(ServeError::Config(
                    format!("native .so plugin {} refused (not a sandbox)", man.name).into(),
                ));
            }
        }
    }
    let _ = Arc::clone(&router.cfg);
    Ok(loaded)
}

/// Re-read `cfg.plugin_dir` if set. Rules stay on their own `rules.reload` atom.
pub fn reload(router: &Router) -> Result<Vec<String>, ServeError> {
    let Some(dir) = router.cfg.plugin_dir.as_ref() else {
        return Ok(Vec::new());
    };
    load_dir(router, dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::rules::Ruleset;
    use crate::static_router;

    #[test]
    fn native_so_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("evil.json"),
            br#"{"name":"x","kind":"native","path":"x.so"}"#,
        )
        .unwrap();
        let cfg =
            Config::from_json(br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864}"#).unwrap();
        let rules = Ruleset::parse(
            br#"{"rules":[{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":[]}]}"#,
        )
        .unwrap();
        let (router, _, _) = static_router(cfg, rules);
        let e = load_dir(&router, dir.path()).unwrap_err();
        let s = e.to_string();
        assert!(s.contains("refused"), "{s}");
    }

    #[cfg(not(feature = "wasm"))]
    #[test]
    fn wasm_kind_without_feature_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("echo.json"),
            br#"{"name":"echo","kind":"wasm","path":"echo.wasm"}"#,
        )
        .unwrap();
        let cfg =
            Config::from_json(br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864}"#).unwrap();
        let rules = Ruleset::parse(
            br#"{"rules":[{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":[]}]}"#,
        )
        .unwrap();
        let (router, _, _) = static_router(cfg, rules);
        let e = load_dir(&router, dir.path()).unwrap_err();
        let s = e.to_string();
        assert!(s.contains("host") || s.contains("wasm"), "{s}");
    }

    #[cfg(feature = "wasm")]
    #[test]
    fn wasm_kind_missing_component_is_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("echo.json"),
            br#"{"name":"echo","kind":"wasm","path":"echo.wasm"}"#,
        )
        .unwrap();
        let cfg =
            Config::from_json(br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864}"#).unwrap();
        let rules = Ruleset::parse(
            br#"{"rules":[{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":[]}]}"#,
        )
        .unwrap();
        let (router, _, _) = static_router(cfg, rules);
        let e = load_dir(&router, dir.path()).unwrap_err();
        let s = e.to_string();
        assert!(s.contains("wasm") || s.contains("config"), "{s}");
    }
}
