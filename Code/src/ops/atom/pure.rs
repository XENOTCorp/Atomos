//! Pure atoms. No world write.
use std::sync::atomic::Ordering;
use serde_json::{json, Value};
use super::AtomCtx;
use crate::align::{STATE_ON, STATE_RESTARTING};
use crate::error::AtomError;
use crate::governor::Governor;
use crate::rules::{RuleError, Ruleset};

pub(crate) fn signal_get(ctx: &AtomCtx) -> Result<Value, AtomError> {
    let s = ctx.signal.v.load(Ordering::Acquire);
    let state = match s {
        STATE_ON => "on",
        STATE_RESTARTING => "restarting",
        _ => "off",
    };
    Ok(json!({ "state": state }))
}

pub(crate) fn json_pretty(input: Value) -> Result<Value, AtomError> {
    let s = serde_json::to_string_pretty(&input)
        .map_err(|e| AtomError::Json(e.to_string().into_boxed_str()))?;
    if s.len() > 1024 * 1024 {
        return Err(AtomError::Bound);
    }
    Ok(json!({ "text": s }))
}

pub(crate) fn resource_get(ctx: &AtomCtx) -> Result<Value, AtomError> {
    Ok(json!({
        "rss_bytes": Governor::rss_bytes(),
        "cpu_fraction": Governor::cpu_fraction(ctx.started),
        "uptime_ms": ctx.started.elapsed().as_millis() as u64
    }))
}

pub(crate) fn rules_dry(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
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

pub(crate) fn overlap_err(e: RuleError) -> Result<Value, AtomError> {
    match e {
        RuleError::Overlap { a, b, example_path } => Ok(json!({
            "ok": false,
            "a": a,
            "b": b,
            "example_path": example_path
        })),
        other => Err(AtomError::Json(other.to_string().into_boxed_str())),
    }
}
