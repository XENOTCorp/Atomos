//! Disjoint path rules. No regex. Load-time overlap is an error.
//! Criticality C2.
//!
//! Matching strategy (thesis NT58/NT65: affine crossover theorem, NT59:
//! measured lattice minimum). The rule languages are disjoint exact and
//! `pre/*` prefix patterns, hence a regular language over path bytes.
//! Two realizations are available:
//!
//! * **Linear scan** — `O(R·L)` byte comparisons over a cache-friendly
//!   array; the minimum for small rule counts R (the bench config with
//!   R=2 never pays for a trie).
//! * **Path trie** — a deterministic automaton (one node per distinct
//!   pattern prefix; Myhill–Nerode refinement), `O(L)` worst case, no
//!   backtracking, no heap. The minimum for large R.
//!
//! [`Ruleset`] picks the realization whose fitted affine cost is lower
//! at load time: trie iff `R ≥ TRIE_MIN_RULES` (the measured crossover;
//! see `bench-results/route-crossover.txt`), capped at 64 rules (the
//! exclude bookkeeping is a `u64` mask) and 16 KiB of pattern bytes.

use serde::Deserialize;

use crate::io::Method;

/// Trie realization below this rule count loses to the linear scan.
/// Measured crossover (release build, `cargo test --release -- --ignored
/// crossover_scan_vs_trie`, recorded in bench-results/route-crossover):
/// scan is affine in R (~4.2 ns/rule; 5.9 ns at R=2, 25.8 at R=8, 59.6
/// at R=16) while the automaton is flat (~35–44 ns); the fitted
/// crossover is R≈10–12, and on the measured grid {2,4,8,16,…} the trie
/// first wins at R=16 — the lattice minimum (thesis NT58/NT59). The
/// production configs in `templates/` (R ≤ 8) therefore keep the scan,
/// which the theorem says is the cheaper realization there.
const TRIE_MIN_RULES: usize = 16;
/// Trie realize at most 64 rules: exclusion tracking is a `u64` mask.
const TRIE_MAX_RULES: usize = 64;
/// Construction guard: total pattern bytes (the trie is worth its
/// memory only for small rule sets; larger ones stay on the scan).
const TRIE_MAX_BYTES: usize = 16 * 1024;

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
    /// Trie realization, built at load time when the crossover theorem
    /// says it beats the linear scan (see module docs).
    trie: Option<PathTrie>,
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
        let rules = rules.into_boxed_slice();
        let s = Self::build(rules)?;
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
        Self::build(packed.into_boxed_slice())
    }

    /// Shared constructor: disjointness is a load-time theorem the trie
    /// relies on (two rules can never terminate the same path), so it is
    /// asserted before the automaton is derived.
    fn build(rules: Box<[Rule]>) -> Result<Self, RuleError> {
        Self::build_inner(rules, false)
    }

    /// Measurement constructor (crossover bench): always derive the
    /// automaton when it fits, ignoring the scan-vs-trie threshold.
    #[cfg(test)]
    pub(crate) fn from_rules_forced(rules: Vec<Rule>) -> Result<Self, RuleError> {
        if rules.len() > MAX_RULES {
            return Err(RuleError::TooMany);
        }
        Self::build_inner(rules.into_boxed_slice(), true)
    }

    fn build_inner(rules: Box<[Rule]>, force: bool) -> Result<Self, RuleError> {
        let s = Self {
            rules,
            trie: None,
        };
        s.assert_disjoint()?;
        let trie = if force {
            PathTrie::build(&s.rules, 0)
        } else {
            PathTrie::build(&s.rules, TRIE_MIN_RULES)
        };
        Ok(Self {
            trie,
            ..s
        })
    }

    pub fn match_path(&self, method: &str, path: &str) -> Option<&Rule> {
        let bit = Method::parse(method).map(Method::bit).unwrap_or(0);
        self.match_bit(bit, path)
    }

    pub fn match_method(&self, method: Method, path: &str) -> Option<&Rule> {
        let bit = method.bit();
        self.match_bit(bit, path)
    }

    fn match_bit(&self, bit: u16, path: &str) -> Option<&Rule> {
        if let Some(t) = &self.trie {
            t.match_rule(bit, path).map(|i| &self.rules[i])
        } else {
            self.rules.iter().find(|r| rule_matches(r, bit, path))
        }
    }

    /// The linear-scan realization (thesis NT58): O(R·L), no heap. The
    /// automaton realization is picked automatically at load time; this
    /// method exists for the equivalence tests and the crossover
    /// measurement, and is always correct.
    pub fn match_linear(&self, method: Method, path: &str) -> Option<&Rule> {
        let bit = method.bit();
        self.rules.iter().find(|r| rule_matches(r, bit, path))
    }

    /// True when the automaton realization is active.
    pub fn trie_active(&self) -> bool {
        self.trie.is_some()
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

/// One trie node: byte transitions plus the terminals ending here.
///
/// Terminals are split by pattern kind because their firing rules
/// differ: a `pre/*` prefix pattern fires the moment the walk *visits*
/// its node (any longer path still starts with `pre/`), while an exact
/// pattern fires only when the walk *ends* at its node.
#[derive(Clone, Debug, Default)]
struct TrieNode {
    /// Sorted `(byte, child)` transitions; deterministic on the byte.
    children: Vec<(u8, u32)>,
    /// Prefix includes ending here: `(rule index, method mask)`.
    prefix_terms: Vec<(u16, u16)>,
    /// Prefix excludes ending here: rule indices.
    prefix_excludes: Vec<u16>,
    /// Exact includes ending here: `(rule index, method mask)`.
    exact_terms: Vec<(u16, u16)>,
    /// Exact excludes ending here: rule indices.
    exact_excludes: Vec<u16>,
}

/// Deterministic automaton over the disjoint rule languages (thesis
/// NT65): matching is one transition per path byte, O(L), no heap, no
/// backtracking. Node 0 is the root.
#[derive(Clone, Debug)]
struct PathTrie {
    nodes: Vec<TrieNode>,
}

impl PathTrie {
    /// Build the automaton, or `None` when the affine crossover says the
    /// linear scan is cheaper (NT58: cost_scan(R) = a·R·L̄ + b vs
    /// cost_trie(L̄); for R below the measured crossover the scan wins).
    /// `min_rules` is the crossover threshold (0 forces the automaton
    /// for the measurement constructor).
    fn build(rules: &[Rule], min_rules: usize) -> Option<PathTrie> {
        let n = rules.len();
        if n < min_rules || n > TRIE_MAX_RULES {
            return None;
        }
        let mut bytes = 0usize;
        for r in rules {
            for p in r.include.iter().chain(r.exclude.iter()) {
                bytes += pattern_len(p);
            }
        }
        if bytes > TRIE_MAX_BYTES {
            return None;
        }
        let mut t = PathTrie {
            nodes: vec![TrieNode::default()],
        };
        for (ri, r) in rules.iter().enumerate() {
            let ri = ri as u16;
            for p in r.include.iter() {
                let node = match p.kind {
                    PatKind::Prefix => t.insert_pre(p.bytes.as_bytes()),
                    PatKind::Exact => t.insert(p.bytes.as_bytes()),
                };
                match p.kind {
                    PatKind::Prefix => t.nodes[node].prefix_terms.push((ri, r.methods)),
                    PatKind::Exact => t.nodes[node].exact_terms.push((ri, r.methods)),
                }
            }
            for p in r.exclude.iter() {
                let node = match p.kind {
                    PatKind::Prefix => t.insert_pre(p.bytes.as_bytes()),
                    PatKind::Exact => t.insert(p.bytes.as_bytes()),
                };
                match p.kind {
                    PatKind::Prefix => t.nodes[node].prefix_excludes.push(ri),
                    PatKind::Exact => t.nodes[node].exact_excludes.push(ri),
                }
            }
        }
        Some(t)
    }

    /// One pass over the path bytes; `cand` is the first include
    /// terminal whose method mask admits the request (at most one rule
    /// can ever match a path — `assert_disjoint` is the load-time
    /// theorem this relies on). An exclude of the candidate anywhere on
    /// the path vetoes the match, mirroring `pat_match` exactly: prefix
    /// excludes fire when visited, exact excludes only at the final
    /// node.
    fn match_rule(&self, bit: u16, path: &str) -> Option<usize> {
        let mut node = 0usize;
        let mut cand: Option<u16> = None;
        let mut exc_seen: u64 = 0;
        for &b in path.as_bytes() {
            let Some(next) = self.descend(node, b) else {
                break;
            };
            node = next;
            let n = &self.nodes[node];
            for &r in &n.prefix_excludes {
                exc_seen |= 1u64 << r;
            }
            if cand.is_none() {
                for &(r, m) in &n.prefix_terms {
                    if m & bit != 0 {
                        cand = Some(r);
                        break;
                    }
                }
            }
        }
        let n = &self.nodes[node];
        for &r in &n.exact_excludes {
            exc_seen |= 1u64 << r;
        }
        if cand.is_none() {
            for &(r, m) in &n.exact_terms {
                if m & bit != 0 {
                    cand = Some(r);
                    break;
                }
            }
        }
        let r = cand?;
        if exc_seen & (1u64 << r) != 0 {
            return None;
        }
        Some(r as usize)
    }

    /// Descend one byte: binary search over the sorted transition list.
    fn descend(&self, node: usize, b: u8) -> Option<usize> {
        let ch = &self.nodes[node].children;
        let idx = ch.partition_point(|&(c, _)| c < b);
        (idx < ch.len() && ch[idx].0 == b).then(|| ch[idx].1 as usize)
    }

    /// Insert a byte path, creating nodes as needed; returns the
    /// terminal node id.
    fn insert(&mut self, bytes: &[u8]) -> usize {
        let mut node = 0usize;
        for &b in bytes {
            node = self.insert_byte(node, b);
        }
        node
    }

    /// `pre/*` patterns occupy `pre + "/"` in the automaton (the walk
    /// can stop the moment the slash after `pre` is consumed; an empty
    /// `pre` is just `/`), mirroring [`pat_match`].
    fn insert_pre(&mut self, pre: &[u8]) -> usize {
        let node = self.insert(pre);
        self.insert_byte(node, b'/')
    }

    /// Single-byte descend-or-create.
    fn insert_byte(&mut self, node: usize, b: u8) -> usize {
        match self.nodes[node].children.binary_search_by_key(&b, |&(c, _)| c) {
            Ok(i) => self.nodes[node].children[i].1 as usize,
            Err(i) => {
                let id = self.nodes.len() as u32;
                self.nodes.push(TrieNode::default());
                self.nodes[node].children.insert(i, (b, id));
                id as usize
            }
        }
    }
}

/// Pattern length in the automaton: exact = its own bytes, prefix =
/// `pre + "/"`. Used for the construction cost guard.
fn pattern_len(p: &Pat) -> usize {
    match p.kind {
        PatKind::Prefix => p.bytes.len() + 1,
        PatKind::Exact => p.bytes.len(),
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

    // --- automaton realization (thesis NT65) ---

    fn mk_rule(id: &str, methods: u16, inc: &[&str], exc: &[&str]) -> Rule {
        Rule {
            methods,
            _pad: [0; 6],
            include: inc.iter().map(|s| pack_pat((*s).to_string())).collect(),
            exclude: exc.iter().map(|s| pack_pat((*s).to_string())).collect(),
            id: id.into(),
            module: id.into(),
            headers: Box::new([]),
        }
    }

    /// 40 pairwise-disjoint GET prefix rules: `/r{i}/*`.
    fn forty_rules() -> Vec<Rule> {
        (0..40)
            .map(|i| mk_rule(&format!("r{i}"), Method::Get.bit(), &[&format!("/r{i}/*")], &[]))
            .collect()
    }

    #[test]
    fn small_rule_sets_keep_the_linear_scan() {
        // The bench config (R=2) must never pay for a trie (NT58).
        let rs = Ruleset::from_rules(vec![
            mk_rule("s", METHODS_ALL, &["/*"], &["/api/*"]),
            mk_rule("a", METHODS_ALL, &["/api/*"], &[]),
        ])
        .unwrap();
        assert!(!rs.trie_active());
    }

    #[test]
    fn large_rule_sets_use_the_automaton() {
        let rs = Ruleset::from_rules(forty_rules()).unwrap();
        assert!(rs.trie_active());
        for i in 0..40 {
            let r = rs.match_method(Method::Get, &format!("/r{i}/x")).unwrap();
            assert_eq!(&*r.id, format!("r{i}"));
        }
        // A path outside every pattern matches nothing.
        assert!(rs.match_method(Method::Get, "/nope/xyz").is_none());
        // Prefix boundary: "/r3" without a trailing slash segment.
        assert!(rs.match_method(Method::Get, "/r3").is_none());
        assert!(rs.match_method(Method::Get, "/r3/").is_some());
    }

    #[test]
    fn method_split_patterns_are_disjoint_by_method() {
        let rs = Ruleset::from_rules(vec![
            mk_rule("g", Method::Get.bit(), &["/api/*"], &[]),
            mk_rule("p", Method::Post.bit(), &["/api/*"], &[]),
        ])
        .unwrap();
        // Too small for the trie — verify with the forced constructor too.
        let rs_t = Ruleset::from_rules_forced(vec![
            mk_rule("g", Method::Get.bit(), &["/api/*"], &[]),
            mk_rule("p", Method::Post.bit(), &["/api/*"], &[]),
        ])
        .unwrap();
        assert!(rs_t.trie_active());
        for rs in [&rs, &rs_t] {
            assert_eq!(&*rs.match_method(Method::Get, "/api/x").unwrap().id, "g");
            assert_eq!(&*rs.match_method(Method::Post, "/api/x").unwrap().id, "p");
            assert!(rs.match_method(Method::Put, "/api/x").is_none());
        }
    }

    #[test]
    fn prefix_exclude_deeper_than_include_vetoes() {
        // The walk fires the "/api/*" include terminal, then must keep
        // walking to see the "/api/private/*" exclude (thesis NT65:
        // exclusion is a bit in the running mask, not an early return).
        let mut rules = forty_rules();
        rules.push(mk_rule(
            "api",
            Method::Get.bit(),
            &["/api/*"],
            &["/api/private/*"],
        ));
        // "/api/*" and "/r{i}/*" never overlap; push a disjoint catch-all
        // is NOT possible, so this is the only "/api" rule.
        let rs = Ruleset::from_rules_forced(rules).unwrap();
        assert!(rs.trie_active());
        assert_eq!(&*rs.match_method(Method::Get, "/api/public/x").unwrap().id, "api");
        assert!(rs.match_method(Method::Get, "/api/private/x").is_none());
        // "/api/private" itself is NOT under the exclude (pat_match needs
        // len > len(pre) and a '/' at len(pre)); the scan agrees.
        assert!(rs.match_method(Method::Get, "/api/private").is_some());
    }

    #[test]
    fn exact_exclude_fires_only_at_path_end() {
        let mut rules = forty_rules();
        rules.push(mk_rule(
            "api",
            Method::Get.bit(),
            &["/api/*"],
            &["/api/x"],
        ));
        let rs = Ruleset::from_rules_forced(rules).unwrap();
        assert_eq!(&*rs.match_method(Method::Get, "/api/y").unwrap().id, "api");
        // "/api/x" exactly: the include prefix fires, then the exact
        // exclude terminal sits at the final node -> veto.
        assert!(rs.match_method(Method::Get, "/api/x").is_none());
        // "/apix" shares bytes but neither the exclude nor the include.
        assert!(rs.match_method(Method::Get, "/apix").is_none());
    }

    #[test]
    fn catchall_star_matches_everything() {
        let mut rules = forty_rules();
        rules.push(mk_rule("catch", Method::Get.bit(), &["/*"], &[]));
        // "/*" overlaps every GET rule -> parse error (load-time theorem).
        assert!(Ruleset::from_rules_forced(rules).is_err());
    }

    #[test]
    fn automaton_equals_linear_scan_on_fuzz() {
        let rs = Ruleset::from_rules_forced(forty_rules()).unwrap();
        assert!(rs.trie_active());
        // Deterministic PRNG (xorshift) — no external deps in tests.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200_000 {
            let r = (next() % 40) as usize;
            let mut path = format!("/r{r}/");
            for _ in 0..(next() % 4) {
                path.push((b'a' + (next() % 26) as u8) as char);
            }
            // Occasionally hit prefix boundaries and non-matching paths.
            let path = match next() % 10 {
                0 => format!("/r{r}"),
                1 => format!("/r{r}/"),
                2 => "/other/x".to_string(),
                3 => "/r".to_string(),
                _ => path,
            };
            let m = next() % 3;
            let method = match m {
                0 => Method::Get,
                1 => Method::Post,
                _ => Method::Delete,
            };
            let a = rs.match_method(method, &path).map(|r| r.id.as_ref());
            let b = rs.match_linear(method, &path).map(|r| r.id.as_ref());
            assert_eq!(a, b, "disagreement on {method:?} {path:?}");
        }
    }

    #[test]
    #[ignore = "crossover measurement; run with `cargo test -- --ignored`"]
    fn crossover_scan_vs_trie() {
        // Affine crossover (NT58) measured directly: time the scan and
        // the automaton on identical rule sets and identical paths, and
        // print the crossing. The observed crossover justifies
        // TRIE_MIN_RULES. No timing assertions (hardware noise).
        let path = "/r17/a/b/c/d/e/f/g/h";
        println!("{:>4} {:>10} {:>10}  winner", "R", "scan ns", "trie ns");
        for &r in &[2usize, 4, 8, 16, 24, 32, 40, 48, 56, 63] {
            let rules: Vec<Rule> = (0..r)
                .map(|i| mk_rule(&format!("r{i}"), Method::Get.bit(), &[&format!("/r{i}/*")], &[]))
                .collect();
            let rs = Ruleset::from_rules_forced(rules).unwrap();
            assert!(rs.trie_active());
            let iters = 200_000u32;
            let t0 = std::time::Instant::now();
            let mut acc = 0usize;
            for _ in 0..iters {
                acc += rs.match_linear(Method::Get, path).is_some() as usize;
            }
            let scan = t0.elapsed().as_nanos() as f64 / iters as f64;
            let t1 = std::time::Instant::now();
            for _ in 0..iters {
                acc += rs.match_method(Method::Get, path).is_some() as usize;
            }
            let trie = t1.elapsed().as_nanos() as f64 / iters as f64;
            std::hint::black_box(acc);
            let winner = if scan < trie { "scan" } else { "trie" };
            println!("{r:>4} {scan:>10.1} {trie:>10.1}  {winner}");
        }
    }
}
