//! Shared request admission, hooks, rule constraints and response finalization.
use std::sync::atomic::Ordering;

use crate::cache::{CacheStamp, ResponseCache};
use crate::config::MemoryMode;
use crate::error::ServeError;
use crate::flags::{FLAG_DEGRADED, FLAG_NO_POST};
use crate::io::{In, Out, OutBody};
use crate::module::Handler;
use crate::sched::ReqGuard;
use crate::status::Status;

use super::Router;

impl Router {
    pub(super) fn begin_request(&self, req: &mut In<'_>) -> Result<ReqGuard, Out> {
        let (guard, over_mem) = self.request_guard(req.peer)?;
        if over_mem {
            req.flags.insert(FLAG_DEGRADED);
        }
        Ok(guard)
    }

    pub(super) fn request_guard(
        &self,
        peer: std::net::SocketAddr,
    ) -> Result<(ReqGuard, bool), Out> {
        self.metrics.requests.v.fetch_add(1, Ordering::Relaxed);
        let guard = self
            .admit(peer)
            .ok_or_else(|| self.err_out(ServeError::Capacity, "scheduler"))?;
        let over_mem = self.gov.over_mem();
        if over_mem && self.gov.mode == MemoryMode::Hard {
            return Err(self.err_out(ServeError::Capacity, "resource bound"));
        }
        Ok((guard, over_mem))
    }

    /// An early response (cache hit or error) is returned through Err so both
    /// invocation paths can share this pipeline without boxing a future.
    pub(super) fn prepare(&self, req: &mut In<'_>) -> Result<(Handler, CacheStamp), Out> {
        if self.pre.is_none() && self.post.is_none() {
            if let Some(hit) = self.cache.get_for(req) {
                self.metrics.hits.v.fetch_add(1, Ordering::Relaxed);
                return Err(hit);
            }
        }
        let stamp = self.cache.stamp();
        if let Some(pre) = &self.pre {
            let out = pre
                .handle(req)
                .map_err(|error| self.err_out(error, "pre"))?;
            if out.status.as_u16() >= 400 {
                return Err(out);
            }
            req.flags.0 |= out.flags.0;
        }
        let rules = self.rules.load();
        let rule = rules
            .match_method(req.method, req.path)
            .ok_or_else(|| self.err_out(ServeError::NoRule, "no rule"))?;
        if let Some(error) = header_fail(rule, req) {
            return Err(self.err_out(error, "header rule"));
        }
        self.modules
            .load()
            .get(rule.module.as_ref())
            .cloned()
            .map(|handler| (handler, stamp))
            .ok_or_else(|| self.err_out(ServeError::Module(rule.module.clone()), "missing module"))
    }

    pub(super) fn finish(
        &self,
        req: &In<'_>,
        result: Result<Out, ServeError>,
        stamp: &CacheStamp,
    ) -> Out {
        let mut out = match result {
            Ok(out) => out,
            Err(error) => return self.track_bytes(self.err_out(error, "module")),
        };
        if let Some(post) = &self.post {
            if !req.flags.contains(FLAG_NO_POST) && !out.flags.contains(FLAG_NO_POST) {
                match post.handle(req) {
                    Ok(result) if result.status.as_u16() != 0 => {
                        if !matches!(result.body, OutBody::Empty) {
                            out.body = result.body;
                        }
                        if result.status != Status::OK {
                            out.status = result.status;
                        }
                        out.flags.0 |= result.flags.0;
                        out.headers.extend(result.headers);
                    }
                    Ok(_) => {}
                    Err(error) => return self.track_bytes(self.err_out(error, "post")),
                }
            }
        }
        // Rules with request-specific constraints cannot use a cache whose
        // key contains only method/path/query. Check this on insertion only.
        if self.pre.is_none()
            && self.post.is_none()
            && ResponseCache::request_is_cacheable(req)
            && self
                .rules
                .load()
                .match_method(req.method, req.path)
                .is_some_and(|rule| rule.headers.is_empty())
        {
            self.cache
                .put_stamped(req.method, req.path, req.query, &out, stamp);
        }
        self.track_bytes(out)
    }

    pub(super) fn track_bytes(&self, out: Out) -> Out {
        self.metrics
            .bytes_out
            .v
            .fetch_add(out.body.len() as u64, Ordering::Relaxed);
        out
    }

    pub(super) fn err_out(&self, error: ServeError, detail: &str) -> Out {
        let status = Status::from_u16(error.status());
        Out::raw(
            status,
            self.errors.render(status, detail),
            "text/html; charset=utf-8",
        )
    }
}

fn header_fail(rule: &crate::rules::Rule, req: &In<'_>) -> Option<ServeError> {
    for header in &rule.headers {
        if header
            .exists
            .is_some_and(|exists| exists != req.headers.get(&header.name).is_some())
        {
            return Some(if header.on_fail == Some(401) {
                ServeError::Unauthorized
            } else {
                ServeError::Forbidden
            });
        }
        if header
            .cidr
            .as_ref()
            .is_some_and(|cidr| !in_cidr(req.peer.ip(), cidr))
        {
            return Some(ServeError::Forbidden);
        }
    }
    None
}

/// Invalid or cross-family CIDRs fail closed. Parsing uses stack storage;
/// these optional rule constraints do not affect unconstrained hot routes.
fn in_cidr(peer: std::net::IpAddr, cidr: &str) -> bool {
    use std::net::IpAddr;
    let Some((network, prefix)) = cidr.split_once('/') else {
        return false;
    };
    let (Ok(network), Ok(prefix)) = (network.parse::<IpAddr>(), prefix.parse::<u32>()) else {
        return false;
    };
    match (peer, network) {
        (IpAddr::V4(peer), IpAddr::V4(network)) if prefix <= 32 => {
            let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
            u32::from(peer) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(peer), IpAddr::V6(network)) if prefix <= 128 => {
            let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
            u128::from(peer) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidrs_match_both_families_and_fail_closed() {
        for (ip, cidr, expected) in [
            ("10.2.3.4", "10.0.0.0/8", true),
            ("10.2.3.4", "11.0.0.0/8", false),
            ("10.2.3.4", "0.0.0.0/0", true),
            ("10.2.3.4", "10.2.3.4/32", true),
            ("10.2.3.4", "10.2.3.4/33", false),
            ("10.2.3.4", "invalid", false),
            ("2001:db8::1", "2001:db8::/32", true),
            ("2001:db9::1", "2001:db8::/32", false),
            ("::1", "::/0", true),
            ("::1", "::1/128", true),
            ("::1", "::/129", false),
            ("::1", "127.0.0.0/8", false),
        ] {
            assert_eq!(in_cidr(ip.parse().unwrap(), cidr), expected, "{ip} {cidr}");
        }
    }
}
