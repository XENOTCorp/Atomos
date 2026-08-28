//! Linear scan matcher. Allocation-free path walk.
use super::parse::{Pat, PatKind, Rule, METHODS_ALL};
use crate::io::Method;

/// No heap. `pre/*` → path starts with `pre/` (or any `/…` if pre is empty).
pub(crate) fn pat_match(p: &Pat, path: &str) -> bool {
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

pub(crate) fn rule_matches(r: &Rule, method_bit: u16, path: &str) -> bool {
    if r.methods != METHODS_ALL && (r.methods & method_bit) == 0 {
        return false;
    }
    if !r.include.iter().any(|p| pat_match(p, path)) {
        return false;
    }
    !r.exclude.iter().any(|p| pat_match(p, path))
}

pub(crate) fn methods_disjoint(a: u16, b: u16) -> bool {
    if a == METHODS_ALL || b == METHODS_ALL {
        return false;
    }
    a & b == 0
}

pub(crate) fn shared_bit(a: u16, b: u16) -> u16 {
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

pub(crate) fn candidates(r: &Rule) -> Vec<String> {
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
