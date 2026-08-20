//! Disjoint path rules. No regex. Load-time overlap is an error.
//! Runtime match O(R). Criticality C2.

use serde::Deserialize;

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
pub struct Rule {
    pub id: String,
    pub module: String,
    #[serde(default)]
    pub methods: Vec<String>,
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub headers: Vec<HeaderRule>,
}

#[derive(Clone, Debug, Deserialize)]
struct File {
    rules: Vec<Rule>,
}

#[derive(Clone, Debug)]
pub struct Ruleset {
    pub rules: Vec<Rule>,
}

const MAX_RULES: usize = 256;
const MAX_PAT: usize = 1024;

impl Ruleset {
    pub fn parse(raw: &[u8]) -> Result<Self, RuleError> {
        let f: File = serde_json::from_slice(raw)
            .map_err(|e| RuleError::Json(e.to_string().into_boxed_str()))?;
        Self::from_rules(f.rules)
    }

    pub fn from_rules(rules: Vec<Rule>) -> Result<Self, RuleError> {
        if rules.len() > MAX_RULES {
            return Err(RuleError::TooMany);
        }
        for r in &rules {
            if r.id.is_empty() {
                return Err(RuleError::EmptyId);
            }
            for p in r.include.iter().chain(r.exclude.iter()) {
                check_pat(p)?;
            }
        }
        let s = Self { rules };
        s.assert_disjoint()?;
        Ok(s)
    }

    pub fn match_path(&self, method: &str, path: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| rule_matches(r, method, path))
    }

    pub fn assert_disjoint(&self) -> Result<(), RuleError> {
        for i in 0..self.rules.len() {
            for j in (i + 1)..self.rules.len() {
                let a = &self.rules[i];
                let b = &self.rules[j];
                if methods_disjoint(a, b) {
                    continue;
                }
                let method = shared_method(a, b);
                for p in candidates(a).into_iter().chain(candidates(b)) {
                    if rule_matches(a, method, &p) && rule_matches(b, method, &p) {
                        return Err(RuleError::Overlap {
                            a: a.id.clone().into_boxed_str(),
                            b: b.id.clone().into_boxed_str(),
                            example_path: p.into_boxed_str(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
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

fn glob_match(pat: &str, path: &str) -> bool {
    if let Some(pre) = pat.strip_suffix("/*") {
        if pre.is_empty() {
            return path.starts_with('/');
        }
        path.starts_with(&format!("{pre}/"))
    } else {
        pat == path
    }
}

fn rule_matches(r: &Rule, method: &str, path: &str) -> bool {
    if !r.methods.is_empty()
        && !r
            .methods
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method))
    {
        return false;
    }
    let inc = r.include.iter().any(|p| glob_match(p, path));
    if !inc {
        return false;
    }
    !r.exclude.iter().any(|p| glob_match(p, path))
}

fn methods_disjoint(a: &Rule, b: &Rule) -> bool {
    if a.methods.is_empty() || b.methods.is_empty() {
        return false;
    }
    !a.methods
        .iter()
        .any(|m| b.methods.iter().any(|n| m.eq_ignore_ascii_case(n)))
}

fn shared_method<'a>(a: &'a Rule, b: &'a Rule) -> &'a str {
    if a.methods.is_empty() && b.methods.is_empty() {
        return "GET";
    }
    if a.methods.is_empty() {
        return b.methods.first().map(String::as_str).unwrap_or("GET");
    }
    if b.methods.is_empty() {
        return a.methods.first().map(String::as_str).unwrap_or("GET");
    }
    a.methods
        .iter()
        .find(|m| b.methods.iter().any(|n| m.eq_ignore_ascii_case(n)))
        .map(String::as_str)
        .unwrap_or("GET")
}

fn candidates(r: &Rule) -> Vec<String> {
    let mut out = Vec::new();
    for p in &r.include {
        if let Some(pre) = p.strip_suffix("/*") {
            if pre.is_empty() {
                out.push("/".into());
                out.push("/x".into());
                out.push("/api/x".into());
            } else {
                out.push(pre.to_string());
                out.push(format!("{pre}/"));
                out.push(format!("{pre}/x"));
            }
        } else {
            out.push(p.clone());
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
        assert_eq!(r.match_path("GET", "/").unwrap().id, "s");
        assert_eq!(r.match_path("GET", "/api/search").unwrap().id, "a");
    }

    #[test]
    fn post_does_not_hit_get_only() {
        let j = r#"{"rules":[
      {"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":[]}
    ]}"#;
        let r = Ruleset::parse(j.as_bytes()).unwrap();
        assert!(r.match_path("POST", "/").is_none());
    }
}
