//! JSON ruleset parse. Pack wire rules. No match.
use serde::Deserialize;
use crate::io::Method;
use super::RuleError;

#[derive(Clone, Debug, Deserialize)]
pub struct HeaderRule {
    pub name: String,
    #[serde(default)]
    pub exists: Option<bool>,
    #[serde(default)]
    pub cidr: Option<String>,
    #[serde(default)]
    pub on_fail: Option<u16>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RuleWire {
    pub(crate) id: String,
    pub(crate) module: String,
    #[serde(default)]
    pub(crate) methods: Vec<String>,
    pub(crate) include: Vec<String>,
    #[serde(default)]
    pub(crate) exclude: Vec<String>,
    #[serde(default)]
    pub(crate) headers: Vec<HeaderRule>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PatKind {
    Exact,
    /// `pre` for JSON `pre/*`. Empty pre is `/*`.
    Prefix,
}

#[derive(Clone, Debug)]
pub(crate) struct Pat {
    pub(crate) kind: PatKind,
    pub(crate) bytes: Box<str>,
}

/// Runtime rule. First cache line holds method mask + include/exclude slices.
#[repr(C, align(64))]
#[derive(Clone, Debug)]
pub struct Rule {
    /// `0xFFFF` = all methods. Else OR of `Method::bit`.
    pub methods: u16,
    pub(crate) _pad: [u8; 6],
    pub(crate) include: Box<[Pat]>,
    pub(crate) exclude: Box<[Pat]>,
    pub id: Box<str>,
    pub module: Box<str>,
    pub headers: Box<[HeaderRule]>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct File {
    pub(crate) rules: Vec<RuleWire>,
}

pub(crate) const MAX_RULES: usize = 256;
pub(crate) const MAX_PAT: usize = 1024;
pub(crate) const METHODS_ALL: u16 = 0xFFFF;
pub(crate) fn pack_rule(w: RuleWire) -> Rule {
    Rule {
        methods: methods_mask(&w.methods),
        _pad: [0; 6],
        include: w.include.into_iter().map(pack_pat).collect(),
        exclude: w.exclude.into_iter().map(pack_pat).collect(),
        id: w.id.into_boxed_str(),
        module: w.module.into_boxed_str(),
        headers: w.headers.into_boxed_slice(),
    }
}

pub(crate) fn pack_pat(p: String) -> Pat {
    if let Some(pre) = p.strip_suffix("/*") {
        Pat {
            kind: PatKind::Prefix,
            bytes: pre.to_string().into_boxed_str(),
        }
    } else {
        Pat {
            kind: PatKind::Exact,
            bytes: p.into_boxed_str(),
        }
    }
}

pub(crate) fn methods_mask(v: &[String]) -> u16 {
    if v.is_empty() {
        return METHODS_ALL;
    }
    let mut m = 0u16;
    for s in v {
        if let Some(bit) = Method::parse(s).map(Method::bit) {
            m |= bit;
        }
    }
    m
}

pub(crate) fn check_pat(p: &str) -> Result<(), RuleError> {
    if p.is_empty() || p.len() > MAX_PAT || !p.starts_with('/') {
        return Err(RuleError::BadPattern(p.into()));
    }
    if p.contains("**") || p.contains('?') || p.contains('[') {
        return Err(RuleError::BadPattern(p.into()));
    }
    if p.contains('*') && !p.ends_with("/*") {
        return Err(RuleError::BadPattern(p.into()));
    }
    Ok(())
}
