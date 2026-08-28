//! Disjoint path rules. No regex. Load-time overlap is an error.
//! Criticality C2.
//!
//! Two realizations: linear scan for small rule sets, path trie for large
//! ones. [`Ruleset`] picks the cheaper one at load time.

mod parse;
mod scan;
mod trie;

use crate::io::Method;

pub use parse::{HeaderRule, Rule};

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

#[repr(C, align(64))]
#[derive(Clone, Debug)]
pub struct Ruleset {
    pub rules: Box<[Rule]>,
    /// Trie realization, built at load time when the crossover theorem
    /// says it beats the linear scan (see module docs).
    trie: Option<trie::PathTrie>,
}

impl Ruleset {
    pub fn parse(raw: &[u8]) -> Result<Self, RuleError> {
        let f: parse::File = serde_json::from_slice(raw)
            .map_err(|e| RuleError::Json(e.to_string().into_boxed_str()))?;
        Self::from_wire(f.rules)
    }

    pub fn from_rules(rules: Vec<Rule>) -> Result<Self, RuleError> {
        if rules.len() > parse::MAX_RULES {
            return Err(RuleError::TooMany);
        }
        let rules = rules.into_boxed_slice();
        let s = Self::build(rules)?;
        Ok(s)
    }

    fn from_wire(wires: Vec<parse::RuleWire>) -> Result<Self, RuleError> {
        if wires.len() > parse::MAX_RULES {
            return Err(RuleError::TooMany);
        }
        let mut packed = Vec::with_capacity(wires.len());
        for w in wires {
            if w.id.is_empty() {
                return Err(RuleError::EmptyId);
            }
            for p in w.include.iter().chain(w.exclude.iter()) {
                parse::check_pat(p)?;
            }
            packed.push(parse::pack_rule(w));
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
        if rules.len() > parse::MAX_RULES {
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
            trie::PathTrie::build(&s.rules, 0)
        } else {
            trie::PathTrie::build(&s.rules, trie::TRIE_MIN_RULES)
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
            self.rules.iter().find(|r| scan::rule_matches(r, bit, path))
        }
    }

    /// The linear-scan realization: O(R·L), no heap. The
    /// automaton realization is picked automatically at load time; this
    /// method exists for the equivalence tests and the crossover
    /// measurement, and is always correct.
    pub fn match_linear(&self, method: Method, path: &str) -> Option<&Rule> {
        let bit = method.bit();
        self.rules.iter().find(|r| scan::rule_matches(r, bit, path))
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
                if scan::methods_disjoint(a.methods, b.methods) {
                    continue;
                }
                let bit = scan::shared_bit(a.methods, b.methods);
                for p in scan::candidates(a).into_iter().chain(scan::candidates(b)) {
                    if scan::rule_matches(a, bit, &p) && scan::rule_matches(b, bit, &p) {
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

    // --- automaton realization ---

    fn mk_rule(id: &str, methods: u16, inc: &[&str], exc: &[&str]) -> Rule {
        Rule {
            methods,
            _pad: [0; 6],
            include: inc.iter().map(|s| parse::pack_pat((*s).to_string())).collect(),
            exclude: exc.iter().map(|s| parse::pack_pat((*s).to_string())).collect(),
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
        // The bench config (R=2) must never pay for a trie.
        let rs = Ruleset::from_rules(vec![
            mk_rule("s", parse::METHODS_ALL, &["/*"], &["/api/*"]),
            mk_rule("a", parse::METHODS_ALL, &["/api/*"], &[]),
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
        // walking to see the "/api/private/*" exclude (:
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
        // Affine crossover measured directly: time the scan and
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
