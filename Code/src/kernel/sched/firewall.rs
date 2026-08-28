//! Admission firewall predicate.
use super::{IpState, Sched};

/// Optional adaptive firewall hook (binarized/deterministic net).
/// Default is `None` -> the threshold predicate only.
pub trait BnnFirewall: Send + Sync {
    fn predict(&self, features: &[i16; 8]) -> bool;
}

impl Sched {
    /// Firewall precondition: every feature under its threshold, or the
    /// IP is an exception. O(1) comparisons. Demand is fixed-point.
    pub fn firewall_pass(&self, ip: &IpState) -> bool {
        if ip.exception {
            return true;
        }
        ip.demand as i32 <= (self.limits.d_limit << 3)
            && ip.recent <= 1024
            && ip.syns <= 256
            && ip.errs <= 64
    }

    /// Optional BNN inference over the quantized feature vector.
    pub fn firewall_adaptive(&self, key: u32) -> bool {
        let Some(bnn) = &self.bnn else { return true };
        let ip = self.ips.get(&key).copied().unwrap_or_default();
        let f: [i16; 8] = [
            ip.demand.min(255) as i16,
            ip.queued.min(255) as i16,
            ip.conns.min(255) as i16,
            ip.wait_ticks.min(255) as i16,
            ip.recent.min(255) as i16,
            ip.syns.min(255) as i16,
            ip.errs.min(255) as i16,
            0,
        ];
        bnn.predict(&f)
    }
}

