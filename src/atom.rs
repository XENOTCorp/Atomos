//! Atoms: pure or effectful. TUI talks only through these.
//! Criticality C2.

use std::path::{Path, PathBuf}; // Path used by atomic_write
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use serde_json::{json, Value};

use crate::align::{LineAtomicU8, STATE_OFF, STATE_ON, STATE_RESTARTING};
use crate::error::AtomError;
use crate::governor::Governor;
use crate::rules::{RuleError, Ruleset};

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
        "tunnel.apply" => Ok(json!({"ok": false, "error": "unconfigured"})),
        _ => Err(AtomError::Unknown(name.into())),
    }
}

fn signal_get(ctx: &AtomCtx) -> Result<Value, AtomError> {
    let s = ctx.signal.v.load(Ordering::Acquire);
    let state = match s {
        STATE_ON => "on",
        STATE_RESTARTING => "restarting",
        _ => "off",
    };
    Ok(json!({ "state": state }))
}

fn json_pretty(input: Value) -> Result<Value, AtomError> {
    let s = serde_json::to_string_pretty(&input)
        .map_err(|e| AtomError::Json(e.to_string().into_boxed_str()))?;
    if s.len() > 1024 * 1024 {
        return Err(AtomError::Bound);
    }
    Ok(json!({ "text": s }))
}

fn resource_get(ctx: &AtomCtx) -> Result<Value, AtomError> {
    Ok(json!({
        "rss_bytes": Governor::rss_bytes(),
        "cpu_fraction": 0.0,
        "uptime_ms": ctx.started.elapsed().as_millis() as u64
    }))
}

fn rules_dry(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
    let rs = if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
        let raw = std::fs::read(p)?;
        Ruleset::parse(&raw)
    } else if input.get("rules").is_some() {
        let raw = serde_json::to_vec(&input)
            .map_err(|e| AtomError::Json(e.to_string().into_boxed_str()))?;
        Ruleset::parse(&raw)
    } else {
        return match ctx.rules.load().assert_disjoint() {
            Ok(()) => Ok(json!({"ok": true})),
            Err(e) => overlap_err(e),
        };
    };
    match rs {
        Ok(_) => Ok(json!({ "ok": true })),
        Err(e) => overlap_err(e),
    }
}

fn overlap_err(e: RuleError) -> Result<Value, AtomError> {
    match e {
        RuleError::Overlap {
            a,
            b,
            example_path,
        } => Ok(json!({
            "ok": false,
            "a": a,
            "b": b,
            "example_path": example_path
        })),
        other => Err(AtomError::Json(other.to_string().into_boxed_str())),
    }
}

fn rules_reload(ctx: &AtomCtx) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let raw = std::fs::read(&ctx.rules_path)?;
    let rs = Ruleset::parse(&raw).map_err(|e| AtomError::Json(e.to_string().into_boxed_str()))?;
    ctx.rules.store(Arc::new(rs));
    Ok(json!({ "ok": true }))
}

fn server_set(ctx: &AtomCtx, st: u8) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    ctx.signal.v.store(st, Ordering::Release);
    if st == STATE_OFF {
        ctx.stop.v.store(1, Ordering::Release);
    }
    if st == STATE_ON {
        ctx.stop.v.store(0, Ordering::Release);
    }
    signal_get(ctx)
}

const CRUD_MAX: u64 = 8 * 1024 * 1024;

fn json_crud(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let path = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AtomError::Input("path".into()))?;
    let op = input
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AtomError::Input("op".into()))?;
    let pointer = input
        .get("pointer")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AtomError::Input("pointer".into()))?;
    let meta = std::fs::metadata(path)?;
    if meta.len() > CRUD_MAX {
        return Err(AtomError::Bound);
    }
    let raw = std::fs::read(path)?;
    let mut doc: Value = serde_json::from_slice(&raw)
        .map_err(|e| AtomError::Json(e.to_string().into_boxed_str()))?;
    match op {
        "add" => {
            let val = input.get("value").cloned().unwrap_or(Value::Null);
            pointer_add(&mut doc, pointer, val)?;
        }
        "put" => {
            let val = input.get("value").cloned().unwrap_or(Value::Null);
            pointer_put(&mut doc, pointer, val)?;
        }
        "del" => pointer_del(&mut doc, pointer)?,
        _ => return Err(AtomError::Input("op".into())),
    }
    atomic_write(Path::new(path), &doc)?;
    Ok(json!({ "ok": true }))
}

fn settings_backup(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let path = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AtomError::Input("path".into()))?;
    let dest = input
        .get("dest")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AtomError::Input("dest".into()))?;
    let b = std::fs::read(path)?;
    if b.len() as u64 > CRUD_MAX {
        return Err(AtomError::Bound);
    }
    atomic_write_bytes(Path::new(dest), &b)?;
    Ok(json!({ "ok": true, "bytes": b.len() }))
}

fn atomic_write(path: &Path, doc: &Value) -> Result<(), AtomError> {
    let bytes = serde_json::to_vec_pretty(doc)
        .map_err(|e| AtomError::Json(e.to_string().into_boxed_str()))?;
    atomic_write_bytes(path, &bytes)
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), AtomError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn split_pointer(p: &str) -> Result<Vec<String>, AtomError> {
    if p.is_empty() {
        return Ok(Vec::new());
    }
    if !p.starts_with('/') {
        return Err(AtomError::Input("pointer".into()));
    }
    let parts: Vec<String> = p
        .split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect();
    if parts.len() > 8 {
        return Err(AtomError::Bound);
    }
    Ok(parts)
}

fn pointer_add(doc: &mut Value, pointer: &str, val: Value) -> Result<(), AtomError> {
    let parts = split_pointer(pointer)?;
    if parts.is_empty() {
        return Err(AtomError::Input("pointer".into()));
    }
    let (parents, last) = parts.split_at(parts.len() - 1);
    let last = &last[0];
    let slot = walk_mut(doc, parents)?;
    if last == "-" {
        let arr = slot.as_array_mut().ok_or(AtomError::Input("not array".into()))?;
        arr.push(val);
        return Ok(());
    }
    match slot {
        Value::Object(map) => {
            if map.contains_key(last) {
                return Err(AtomError::Conflict);
            }
            map.insert(last.clone(), val);
            Ok(())
        }
        Value::Array(arr) => {
            let i: usize = last.parse().map_err(|_| AtomError::Input("index".into()))?;
            if i > arr.len() {
                return Err(AtomError::NotFound);
            }
            arr.insert(i, val);
            Ok(())
        }
        _ => Err(AtomError::Input("parent".into())),
    }
}

fn pointer_put(doc: &mut Value, pointer: &str, val: Value) -> Result<(), AtomError> {
    let parts = split_pointer(pointer)?;
    if parts.is_empty() {
        *doc = val;
        return Ok(());
    }
    let (parents, last) = parts.split_at(parts.len() - 1);
    let last = &last[0];
    let slot = walk_mut(doc, parents)?;
    match slot {
        Value::Object(map) => {
            map.insert(last.clone(), val);
            Ok(())
        }
        Value::Array(arr) => {
            let i: usize = last.parse().map_err(|_| AtomError::Input("index".into()))?;
            if i >= arr.len() {
                return Err(AtomError::NotFound);
            }
            arr[i] = val;
            Ok(())
        }
        _ => Err(AtomError::Input("parent".into())),
    }
}

fn pointer_del(doc: &mut Value, pointer: &str) -> Result<(), AtomError> {
    let parts = split_pointer(pointer)?;
    if parts.is_empty() {
        return Err(AtomError::Input("pointer".into()));
    }
    let (parents, last) = parts.split_at(parts.len() - 1);
    let last = &last[0];
    let slot = walk_mut(doc, parents)?;
    match slot {
        Value::Object(map) => {
            map.remove(last).ok_or(AtomError::NotFound)?;
            Ok(())
        }
        Value::Array(arr) => {
            let i: usize = last.parse().map_err(|_| AtomError::Input("index".into()))?;
            if i >= arr.len() {
                return Err(AtomError::NotFound);
            }
            arr.remove(i);
            Ok(())
        }
        _ => Err(AtomError::Input("parent".into())),
    }
}

fn walk_mut<'a>(doc: &'a mut Value, parts: &[String]) -> Result<&'a mut Value, AtomError> {
    let mut cur = doc;
    for p in parts {
        cur = match cur {
            Value::Object(map) => map.get_mut(p).ok_or(AtomError::NotFound)?,
            Value::Array(arr) => {
                let i: usize = p.parse().map_err(|_| AtomError::Input("index".into()))?;
                arr.get_mut(i).ok_or(AtomError::NotFound)?
            }
            _ => return Err(AtomError::NotFound),
        };
    }
    Ok(cur)
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
        assert_eq!(rs.match_path("GET", "/api/x").unwrap().id, "a");
        assert_eq!(rs.match_path("GET", "/").unwrap().id, "s");
    }
}
