//! Effectful atoms. World write through files and signal.
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use serde_json::{json, Value};
use super::pure::signal_get;
use super::AtomCtx;
use crate::align::{STATE_OFF, STATE_ON};
use crate::error::AtomError;
use crate::rules::Ruleset;

/// One audit line: `ts, atom, key_id, old_hash, new_hash`.
pub(crate) fn audit_line(ctx: &AtomCtx, atom: &str, key_id: &str, old_hash: &str, new_hash: &str) {
    if ctx.audit_path.as_os_str().is_empty() {
        return;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{ts}, {atom}, {key_id}, {old_hash}, {new_hash}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ctx.audit_path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn hash_bytes(b: &[u8]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    format!("{:x}", h.finish())
}

pub(crate) fn server_drain(ctx: &AtomCtx) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let old = ctx.drain.v.load(Ordering::Acquire);
    ctx.drain.v.store(1, Ordering::Release);
    audit_line(ctx, "server.drain", "listen", &old.to_string(), "1");
    Ok(json!({ "ok": true }))
}

pub(crate) fn audit_append(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let atom = input
        .get("atom")
        .and_then(|v| v.as_str())
        .unwrap_or("audit.append");
    let key_id = input
        .get("key_id")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let old_hash = input
        .get("old_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let new_hash = input
        .get("new_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    audit_line(ctx, atom, key_id, old_hash, new_hash);
    Ok(json!({ "ok": true }))
}

pub(crate) fn cache_purge(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let key = input
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("*");
    if key == "*" {
        ctx.cache.invalidate();
    } else {
        ctx.cache.invalidate_named(key);
    }
    audit_line(ctx, "cache.purge", key, "cache", "empty");
    Ok(json!({ "ok": true }))
}

pub(crate) fn rules_reload(ctx: &AtomCtx) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let raw = std::fs::read(&ctx.rules_path)?;
    let rs = Ruleset::parse(&raw).map_err(|e| AtomError::Json(e.to_string().into_boxed_str()))?;
    let newh = hash_bytes(&raw);
    ctx.rules.store(Arc::new(rs));
    audit_line(ctx, "rules.reload", "rules", "-", &newh);
    Ok(json!({ "ok": true }))
}

pub(crate) fn server_set(ctx: &AtomCtx, st: u8) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let old = ctx.signal.v.load(Ordering::Acquire);
    ctx.signal.v.store(st, Ordering::Release);
    if st == STATE_OFF {
        ctx.stop.v.store(1, Ordering::Release);
    }
    if st == STATE_ON {
        ctx.stop.v.store(0, Ordering::Release);
    }
    audit_line(
        ctx,
        "server.set",
        "signal",
        &old.to_string(),
        &st.to_string(),
    );
    signal_get(ctx)
}

const CRUD_MAX: u64 = 8 * 1024 * 1024;

pub(crate) fn json_crud(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
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
    audit_line(ctx, "json.crud", path, op, pointer);
    Ok(json!({ "ok": true }))
}

pub(crate) fn settings_backup(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
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
    audit_line(
        ctx,
        "settings.backup",
        path,
        &hash_bytes(&b),
        dest,
    );
    Ok(json!({ "ok": true, "bytes": b.len() }))
}

pub(crate) fn atomic_write(path: &Path, doc: &Value) -> Result<(), AtomError> {
    let bytes = serde_json::to_vec_pretty(doc)
        .map_err(|e| AtomError::Json(e.to_string().into_boxed_str()))?;
    atomic_write_bytes(path, &bytes)
}

pub(crate) fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), AtomError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub(crate) fn split_pointer(p: &str) -> Result<Vec<String>, AtomError> {
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

pub(crate) fn pointer_add(doc: &mut Value, pointer: &str, val: Value) -> Result<(), AtomError> {
    let parts = split_pointer(pointer)?;
    if parts.is_empty() {
        return Err(AtomError::Input("pointer".into()));
    }
    let (parents, last) = parts.split_at(parts.len() - 1);
    let last = &last[0];
    let slot = walk_mut(doc, parents)?;
    if last == "-" {
        let arr = slot
            .as_array_mut()
            .ok_or(AtomError::Input("not array".into()))?;
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

pub(crate) fn pointer_put(doc: &mut Value, pointer: &str, val: Value) -> Result<(), AtomError> {
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

pub(crate) fn pointer_del(doc: &mut Value, pointer: &str) -> Result<(), AtomError> {
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

pub(crate) fn walk_mut<'a>(doc: &'a mut Value, parts: &[String]) -> Result<&'a mut Value, AtomError> {
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
