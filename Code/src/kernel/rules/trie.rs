//! Deterministic path automaton.
use super::parse::{Pat, PatKind, Rule};

/// Trie realization below this rule count loses to the linear scan.
/// Measured crossover (release build, `cargo test --release -- --ignored
/// crossover_scan_vs_trie`, recorded in bench-results/route-crossover):
/// scan is affine in R (~4.2 ns/rule; 5.9 ns at R=2, 25.8 at R=8, 59.6
/// at R=16) while the automaton is flat (~35–44 ns); the fitted
/// crossover is R≈10–12, and on the measured grid {2,4,8,16,…} the trie
/// first wins at R=16 — the lattice minimum. The
/// production configs in `templates/` (R ≤ 8) therefore keep the scan,
/// which the theorem says is the cheaper realization there.
pub(crate) const TRIE_MIN_RULES: usize = 16;
/// Trie realize at most 64 rules: exclusion tracking is a `u64` mask.
pub(crate) const TRIE_MAX_RULES: usize = 64;
/// Construction guard: total pattern bytes (the trie is worth its
/// memory only for small rule sets; larger ones stay on the scan).
pub(crate) const TRIE_MAX_BYTES: usize = 16 * 1024;
/// One trie node: byte transitions plus the terminals ending here.
///
/// Terminals are split by pattern kind because their firing rules
/// differ: a `pre/*` prefix pattern fires the moment the walk *visits*
/// its node (any longer path still starts with `pre/`), while an exact
/// pattern fires only when the walk *ends* at its node.
#[derive(Clone, Debug, Default)]
pub(crate) struct TrieNode {
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

/// Deterministic automaton over the disjoint rule languages.
/// Matching is one transition per path byte, O(L), no heap, no
/// backtracking. Node 0 is the root.
#[derive(Clone, Debug)]
pub(crate) struct PathTrie {
    nodes: Vec<TrieNode>,
}

impl PathTrie {
    /// Build the automaton, or `None` when the affine crossover says the
    /// linear scan is cheaper (cost_scan(R) = a·R·L̄ + b vs
    /// cost_trie(L̄); for R below the measured crossover the scan wins).
    /// `min_rules` is the crossover threshold (0 forces the automaton
    /// for the measurement constructor).
    pub(crate) fn build(rules: &[Rule], min_rules: usize) -> Option<PathTrie> {
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
    pub(crate) fn match_rule(&self, bit: u16, path: &str) -> Option<usize> {
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
pub(crate) fn pattern_len(p: &Pat) -> usize {
    match p.kind {
        PatKind::Prefix => p.bytes.len() + 1,
        PatKind::Exact => p.bytes.len(),
    }
}
