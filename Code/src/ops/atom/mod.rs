//! Atoms: pure or effectful. TUI talks only through these.
//! Criticality C2.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use serde_json::{json, Value};

use crate::align::{LineAtomicU8, STATE_OFF, STATE_ON, STATE_RESTARTING};
use crate::cache::ResponseCache;
use crate::error::AtomError;
use crate::rules::Ruleset;

mod pure;
mod effectful;
use effectful::*;
use pure::*;

pub enum AtomKind {
    Pure,
    Effectful,
}

pub trait Atom: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn kind(&self) -> AtomKind;
    fn run(&self, ctx: &AtomCtx, input: Value) -> Result<Value, AtomError>;
}

#[derive(Clone)]
pub struct AtomCtx {
    pub signal: Arc<LineAtomicU8>,
    pub rules: Arc<ArcSwap<Ruleset>>,
    pub rules_path: PathBuf,
    pub started: Instant,
    pub allow_write: bool,
    pub stop: Arc<LineAtomicU8>,
    pub cache: Arc<ResponseCache>,
}

impl AtomCtx {
    pub fn test() -> Self {
        let empty = Ruleset::from_rules(vec![]).unwrap();
        Self {
            signal: Arc::new(LineAtomicU8::new(STATE_OFF)),
            rules: Arc::new(ArcSwap::from_pointee(empty)),
            rules_path: PathBuf::from("/dev/null"),
            started: Instant::now(),
            allow_write: true,
            stop: Arc::new(LineAtomicU8::new(0)),
            cache: Arc::new(ResponseCache::new(8, 64 * 1024)),
        }
    }

    pub fn run(&self, name: &str, input: Value) -> Result<Value, AtomError> {
        dispatch(self, name, input)
    }
}

pub fn dispatch(ctx: &AtomCtx, name: &str, input: Value) -> Result<Value, AtomError> {
    match name {
        "signal.get" => signal_get(ctx),
        "json.pretty" => json_pretty(input),
        "resource.get" => resource_get(ctx),
        "rules.dry_test" => rules_dry(ctx, input),
        "json.crud" => json_crud(ctx, input),
        "settings.backup" => settings_backup(ctx, input),
        "server.start" => server_set(ctx, STATE_ON),
        "server.stop" => server_set(ctx, STATE_OFF),
        "server.restart" => {
            server_set(ctx, STATE_RESTARTING)?;
            server_set(ctx, STATE_OFF)?;
            server_set(ctx, STATE_ON)
        }
        "rules.reload" => rules_reload(ctx),
        "cache.purge" => cache_purge(ctx, input),
        "tunnel.apply" => Ok(json!({"ok": false, "error": "unconfigured"})),
        _ => Err(AtomError::Unknown(name.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_starts_off() {
        let ctx = AtomCtx::test();
        let v = ctx.run("signal.get", json!({})).unwrap();
        assert_eq!(v["state"], "off");
    }

    #[test]
    fn json_crud_add_put_del() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k.json");
        std::fs::write(&p, r#"{"keys":[]}"#).unwrap();
        let ctx = AtomCtx::test();
        ctx.run(
            "json.crud",
            json!({"path": p.to_str().unwrap(), "op":"add", "pointer":"/keys/-", "value":"A"}),
        )
        .unwrap();
        let got: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(got["keys"][0], "A");
        ctx.run(
            "json.crud",
            json!({"path": p.to_str().unwrap(), "op":"put", "pointer":"/keys/0", "value":"B"}),
        )
        .unwrap();
        ctx.run(
            "json.crud",
            json!({"path": p.to_str().unwrap(), "op":"del", "pointer":"/keys/0"}),
        )
        .unwrap();
        let got: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(got["keys"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn restart_is_stop_then_start() {
        let ctx = AtomCtx::test();
        ctx.run("server.restart", json!({})).unwrap();
        let v = ctx.run("signal.get", json!({})).unwrap();
        assert_eq!(v["state"], "on");
    }

    #[test]
    fn rules_reload_swaps_arc() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rules.json");
        std::fs::write(
            &p,
            r#"{"rules":[{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/api/*"]},
                         {"id":"a","module":"api","methods":["GET"],"include":["/api/*"],"exclude":[]}]}"#,
        )
        .unwrap();
        let mut ctx = AtomCtx::test();
        ctx.rules_path = p;
        ctx.run("rules.reload", json!({})).unwrap();
        let rs = ctx.rules.load();
        assert_eq!(rs.match_path("GET", "/api/x").unwrap().id.as_ref(), "a");
        assert_eq!(rs.match_path("GET", "/").unwrap().id.as_ref(), "s");
    }
}
