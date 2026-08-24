//! Disjoint path rules. No regex. Load-time overlap is an error.
//! Runtime match O(R), no heap. Criticality C2.

use serde::Deserialize;

use crate::io::Method;

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("json: {0}")]
    Json(Box<str>),
    #[error("overlap {a} and {b} at {example_path}")]
    Overlap {
        a: Box<str>,
        b: Box<str>,
        example_path: Box<str>,
    },
    #[error("too many rules")]
    TooMany,
    #[error("empty id")]
    EmptyId,
    #[error("bad pattern {0}")]
    BadPattern(Box<str>),
}

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
struct RuleWire {
    id: String,
    module: String,
    #[serde(default)]
    methods: Vec<String>,
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    headers: Vec<HeaderRule>,
}

#[derive(Clone, Copy, Debug)]
enum PatKind {
    Exact,
    /// `pre` for JSON `pre/*`. Empty pre is `/*`.
    Prefix,
}

#[derive(Clone, Debug)]
struct Pat {
    kind: PatKind,
    bytes: Box<str>,
}

/// Runtime rule. First cache line holds method mask + include/exclude slices.
#[repr(C, align(64))]
#[derive(Clone, Debug)]
pub struct Rule {
    /// `0xFFFF` = all methods. Else OR of `Method::bit`.
    pub methods: u16,
    _pad: [u8; 6],
    include: Box<[Pat]>,
    exclude: Box<[Pat]>,
    pub id: Box<str>,
    pub module: Box<str>,
    pub headers: Box<[HeaderRule]>,
}

#[derive(Clone, Debug, Deserialize)]
struct File {
    rules: Vec<RuleWire>,
}

#[repr(C, align(64))]
#[derive(Clone, Debug)]
pub struct Ruleset {
    pub rules: Box<[Rule]>,
}

const MAX_RULES: usize = 256;
const MAX_PAT: usize = 1024;
const METHODS_ALL: u16 = 0xFFFF;

impl Ruleset {
    pub fn parse(raw: &[u8]) -> Result<Self, RuleError> {
        let f: File = serde_json::from_slice(raw)
            .map_err(|e| RuleError::Json(e.to_string().into_boxed_str()))?;
        Self::from_wire(f.rules)
    }

    pub fn from_rules(rules: Vec<Rule>) -> Result<Self, RuleError> {
        if rules.len() > MAX_RULES {
            return Err(RuleError::TooMany);
        }
        let s = Self {
            rules: rules.into_boxed_slice(),
        };
        s.assert_disjoint()?;
        Ok(s)
    }

    fn from_wire(wires: Vec<RuleWire>) -> Result<Self, RuleError> {
        if wires.len() > MAX_RULES {
            return Err(RuleError::TooMany);
        }
        let mut packed = Vec::with_capacity(wires.len());
        for w in wires {
            if w.id.is_empty() {
                return Err(RuleError::EmptyId);
            }
            for p in w.include.iter().chain(w.exclude.iter()) {
                check_pat(p)?;
            }
            packed.push(pack_rule(w));
        }
        let s = Self {
            rules: packed.into_boxed_slice(),
        };
        s.assert_disjoint()?;
        Ok(s)
    }

    pub fn match_path(&self, method: &str, path: &str) -> Option<&Rule> {
        let bit = Method::parse(method).map(Method::bit).unwrap_or(0);
        self.rules.iter().find(|r| rule_matches(r, bit, path))
    }

    pub fn match_method(&self, method: Method, path: &str) -> Option<&Rule> {
        let bit = method.bit();
        self.rules.iter().find(|r| rule_matches(r, bit, path))
    }

    pub fn assert_disjoint(&self) -> Result<(), RuleError> {
        for i in 0..self.rules.len() {
            for j in (i + 1)..self.rules.len() {
                let a = &self.rules[i];
                let b = &self.rules[j];
                if methods_disjoint(a.methods, b.methods) {
                    continue;
                }
                let bit = shared_bit(a.methods, b.methods);
                for p in candidates(a).into_iter().chain(candidates(b)) {
                    if rule_matches(a, bit, &p) && rule_matches(b, bit, &p) {
                        return Err(RuleError::Overlap {
                            a: a.id.clone(),
                            b: b.id.clone(),
                            example_path: p.into_boxed_str(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

fn pack_rule(w: RuleWire) -> Rule {
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

fn pack_pat(p: String) -> Pat {
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

fn methods_mask(v: &[String]) -> u16 {
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

fn check_pat(p: &str) -> Result<(), RuleError> {
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

/// No heap. `pre/*` → path starts with `pre/` (or any `/…` if pre is empty).
fn pat_match(p: &Pat, path: &str) -> bool {
    match p.kind {
        PatKind::Exact => path == &*p.bytes,
        PatKind::Prefix => {
            let pre = p.bytes.as_ref();
            if pre.is_empty() {
                return path.starts_with('/');
            }
            let pb = pre.as_bytes();
            let sb = path.as_bytes();
            sb.len() > pb.len() && sb[pb.len()] == b'/' && sb.starts_with(pb)
        }
    }
}

fn rule_matches(r: &Rule, method_bit: u16, path: &str) -> bool {
    if r.methods != METHODS_ALL && (r.methods & method_bit) == 0 {
        return false;
    }
    if !r.include.iter().any(|p| pat_match(p, path)) {
        return false;
    }
    !r.exclude.iter().any(|p| pat_match(p, path))
}

fn methods_disjoint(a: u16, b: u16) -> bool {
    if a == METHODS_ALL || b == METHODS_ALL {
        return false;
    }
    a & b == 0
}

fn shared_bit(a: u16, b: u16) -> u16 {
    if a == METHODS_ALL && b == METHODS_ALL {
        return Method::Get.bit();
    }
    if a == METHODS_ALL {
        return if b == 0 { Method::Get.bit() } else { 1 << b.trailing_zeros() };
    }
    if b == METHODS_ALL {
        return if a == 0 { Method::Get.bit() } else { 1 << a.trailing_zeros() };
    }
    let both = a & b;
    if both == 0 {
        Method::Get.bit()
    } else {
        1 << both.trailing_zeros()
    }
}

fn candidates(r: &Rule) -> Vec<String> {
    let mut out = Vec::new();
    for p in r.include.iter() {
        match p.kind {
            PatKind::Prefix => {
                let pre = p.bytes.as_ref();
                if pre.is_empty() {
                    out.push("/".into());
                    out.push("/x".into());
                    out.push("/api/x".into());
                } else {
                    out.push(pre.to_string());
                    let mut slash = String::with_capacity(pre.len() + 1);
                    slash.push_str(pre);
                    slash.push('/');
                    out.push(slash);
                    let mut x = String::with_capacity(pre.len() + 2);
                    x.push_str(pre);
                    x.push_str("/x");
                    out.push(x);
                }
            }
            PatKind::Exact => out.push(p.bytes.to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_star_rules_overlap() {
        let j = r#"{"rules":[
      {"id":"a","module":"static","methods":["GET"],"include":["/*"],"exclude":[]},
      {"id":"b","module":"api","methods":["GET"],"include":["/*"],"exclude":[]}
    ]}"#;
        let e = Ruleset::parse(j.as_bytes()).unwrap_err();
        assert!(matches!(e, RuleError::Overlap { .. }));
    }

    #[test]
    fn exclude_makes_disjoint() {
        let j = r#"{"rules":[
      {"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/api/*"]},
      {"id":"a","module":"api","methods":["GET"],"include":["/api/*"],"exclude":[]}
    ]}"#;
        let r = Ruleset::parse(j.as_bytes()).unwrap();
        assert_eq!(&*r.match_path("GET", "/").unwrap().id, "s");
        assert_eq!(&*r.match_path("GET", "/api/search").unwrap().id, "a");
    }

    #[test]
    fn post_does_not_hit_get_only() {
        let j = r#"{"rules":[
      {"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":[]}
    ]}"#;
        let r = Ruleset::parse(j.as_bytes()).unwrap();
        assert!(r.match_path("POST", "/").is_none());
    }

    #[test]
    fn prefix_does_not_match_without_slash() {
        let j = r#"{"rules":[
      {"id":"a","module":"api","methods":["GET"],"include":["/api/*"],"exclude":[]}
    ]}"#;
        let r = Ruleset::parse(j.as_bytes()).unwrap();
        assert!(r.match_path("GET", "/api").is_none());
        assert!(r.match_path("GET", "/apix").is_none());
        assert!(r.match_path("GET", "/api/").is_some());
        assert!(r.match_path("GET", "/api/health").is_some());
        assert_eq!(
            r.match_method(Method::Get, "/api/health").unwrap().id.as_ref(),
            "a"
        );
    }

    #[test]
    fn packed_rule_is_cache_line_aligned() {
        assert_eq!(std::mem::align_of::<Rule>(), 64);
        assert_eq!(std::mem::align_of::<Ruleset>(), 64);
        assert!(std::mem::size_of::<Rule>() >= 64);
    }
}
