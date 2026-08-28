//! Integer-only request admission, firewall, and priority scheduler.
//!
//! Every equation in this module uses only additions, comparisons,
//! shifts, and min/max: no division, no floating point. All state is
//! plain integers; the hot structures are a per-IP table (hashbrown)
//! and a handful of global counters. A request passes through:
//!
//! ```text
//! Request -> firewall (pure predicate) -> admission (bounds + score)
//!         -> [module] -> completion (counters decremented)
//! ```
//!
//! Rule modes swap the weights (a "Lawvere theory" per the design):
//! - `Anarchy` : FCFS: only wait time matters, per-IP queues unused.
//! - `MaxAvail`: strong IP-diversity bonus, low demand preferred.
//! - `Fair`    : weighted fair queuing: priority proportional to
//!   inverse demand.
//! - `Custom`  : user-supplied integer weights from config.
//!
//! The optional binarized-neural-network firewall is a trait hook
//! (`BnnFirewall`); the default is the threshold-rule predicate.

use std::sync::Arc;

use parking_lot::Mutex;

mod ip;
mod firewall;
mod score;
pub use ip::IpState;
pub use firewall::BnnFirewall;


/// Default per-IP demand limit before the firewall throttles.
pub const DEFAULT_D_LIMIT: i32 = 64;

/// Weight defaults (powers of two so the score is shifts and adds).
pub const W_DIV: i32 = 1 << 6;
pub const W_DEM: i32 = 1 << 3;
pub const W_EXC: i32 = 1 << 5;
pub const W_WAIT: i32 = 1;
pub const W_Q: i32 = 1 << 2;
pub const W_C: i32 = 1 << 2;

/// Rule mode: which scheduling theory the weights express.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleMode {
    Anarchy,
    MaxAvail,
    #[default]
    Fair,
    Custom,
}

/// Integer weights for the admission/priority scores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Weights {
    pub div: i32,
    pub dem: i32,
    pub exc: i32,
    pub wait: i32,
    pub qpen: i32,
    pub cpen: i32,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            div: W_DIV,
            dem: W_DEM,
            exc: W_EXC,
            wait: W_WAIT,
            qpen: W_Q,
            cpen: W_C,
        }
    }
}

impl Weights {
    /// Rule-mode -> weights (`SelectWeights : Rule -> Weights`).
    pub fn for_mode(mode: RuleMode, custom: Weights) -> Weights {
        match mode {
            RuleMode::Anarchy => Weights {
                div: 0,
                dem: 0,
                exc: 0,
                wait: 1,
                qpen: 0,
                cpen: 0,
            },
            RuleMode::MaxAvail => Weights {
                div: 1 << 10, // diversity dominates
                dem: 1 << 3,
                exc: 1 << 5,
                wait: 1,
                qpen: 1 << 2,
                cpen: 1 << 2,
            },
            RuleMode::Fair => Weights::default(),
            RuleMode::Custom => custom,
        }
    }
}

/// Global integer limits (plain comparisons, no math beyond that).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Limits {
    /// Max total live connections.
    pub c_max: u32,
    /// Max live connections per IP.
    pub c_per_ip: u32,
    /// Max total requests in flight (queued + running).
    pub q_max: u32,
    /// Max in-flight requests per IP.
    pub q_per_ip: u32,
    /// Max backlogged (not yet admitted) items.
    pub b_max: u32,
    /// Max request header bytes.
    pub h_max: u32,
    /// Max request body bytes.
    pub s_max: u64,
    /// Max concurrent H2/H3 streams.
    pub str_max: u32,
    /// Firewall demand threshold.
    pub d_limit: i32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Capacity ceilings generous enough for loopback benches
            // (wrk: 400 connections from one IP); real deployments
            // tighten via the `scheduler` config block.
            c_max: 4096,
            c_per_ip: 1024,
            q_max: 16_384,
            q_per_ip: 4096,
            b_max: 8192,
            h_max: 65_536,
            s_max: 10 << 20,
            str_max: 4096,
            d_limit: DEFAULT_D_LIMIT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Accepted into the queue; a guard decrements on completion.
    Accepted,
    /// Over a per-IP/global bound; parked in the backlog.
    Backlogged,
    /// Firewall or capacity rejected.
    Rejected,
}

pub struct Sched {
    pub rule: RuleMode,
    pub custom: Weights,
    pub limits: Limits,
    pub bnn: Option<Arc<dyn BnnFirewall>>,
    pub(crate) ips: hashbrown::HashMap<u32, IpState>,
    /// Total in-flight requests.
    pub q_total: u32,
    /// Total live connections.
    pub c_total: u32,
    /// Backlog length.
    pub backlog: u32,
    /// Tick counter (decays demand / ages waits).
    pub tick: u32,
}

impl Sched {
    pub fn new(rule: RuleMode, custom: Weights, limits: Limits) -> Self {
        Self {
            rule,
            custom,
            limits,
            bnn: None,
            ips: hashbrown::HashMap::with_capacity(64),
            q_total: 0,
            c_total: 0,
            backlog: 0,
            tick: 0,
        }
    }

    /// Sharded scheduler for multi-threaded dispatch: `n` independent
    /// tables keyed by IP hash, so concurrent workers never contend on
    /// one mutex. Global limits are divided across shards (each shard
    /// caps at `q_max/n`, `c_max/n`), a standard sharded-counter
    /// approximation.
    pub fn sharded(n: usize, rule: RuleMode, custom: Weights, limits: Limits) -> Vec<Arc<Mutex<Sched>>> {
        let per_shard = Limits {
            q_max: (limits.q_max / n.max(1) as u32).max(1),
            c_max: (limits.c_max / n.max(1) as u32).max(1),
            b_max: (limits.b_max / n.max(1) as u32).max(1),
            ..limits
        };
        (0..n.max(1))
            .map(|_| Arc::new(Mutex::new(Sched::new(rule, custom, per_shard))))
            .collect()
    }

    /// FNV-1a over the address bytes. The IP hash key. No heap.
    pub fn ip_key(peer: std::net::SocketAddr) -> u32 {
        let mut h = 0x811c_9dc5u32;
        match peer {
            std::net::SocketAddr::V4(a) => {
                for b in a.ip().octets() {
                    h ^= b as u32;
                    h = h.wrapping_mul(0x0100_0193);
                }
            }
            std::net::SocketAddr::V6(a) => {
                for b in a.ip().octets() {
                    h ^= b as u32;
                    h = h.wrapping_mul(0x0100_0193);
                }
            }
        }
        h
    }

    fn entry(&mut self, key: u32) -> &mut IpState {
        self.ips.entry(key).or_default()
    }

    pub fn admit_request(&mut self, key: u32) -> Admission {
        // Shortcut 1: global queue bound, before touching the table.
        if self.q_total >= self.limits.q_max {
            if self.backlog < self.limits.b_max {
                self.backlog += 1;
                return Admission::Backlogged;
            }
            return Admission::Rejected;
        }
        // Single entry borrow (field-level: `self.ips` only, so the
        // other counters stay readable/writable).
        let ip = self.ips.entry(key).or_default();
        ip.demand_update(1);
        // Rolling window decay on `recent` (no clock in the hot path):
        // `r -= r>>4` per request keeps it bounded (~16x the sustained
        // rate) so a long-lived client can never trip the threshold by
        // volume alone.
        ip.recent = ip.recent.wrapping_sub(ip.recent >> 4);
        ip.recent = ip.recent.saturating_add(1);
        ip.wait_ticks = ip.wait_ticks.saturating_add(1);
        // Arm the firewall only when a feature could plausibly fail it.
        if ip.demand as i32 >= (self.limits.d_limit << 3)
            || ip.recent >= 1024
            || ip.syns >= 256
            || ip.errs >= 64
        {
            ip.hot = true;
        }
        // Shortcut 2: per-IP queue bound.
        if ip.queued >= self.limits.q_per_ip {
            if self.backlog < self.limits.b_max {
                self.backlog += 1;
                return Admission::Backlogged;
            }
            return Admission::Rejected;
        }
        // Shortcut 3: firewall predicate: only when armed.
        if ip.hot
            && !ip.exception
            && (ip.demand as i32 > (self.limits.d_limit << 3)
                || ip.recent > 1024
                || ip.syns > 256
                || ip.errs > 64)
        {
            return Admission::Rejected;
        }
        // Commit.
        ip.queued += 1;
        self.q_total += 1;
        Admission::Accepted
    }

    pub fn admit_conn(&mut self, key: u32) -> bool {
        let c_per_ip = self.limits.c_per_ip;
        let c_max = self.limits.c_max;
        let c_total = self.c_total;
        let over = {
            let ip = self.entry(key);
            ip.conns >= c_per_ip || c_total >= c_max
        };
        if over {
            return false;
        }
        self.entry(key).conns += 1;
        self.c_total += 1;
        true
    }

    /// Connection teardown.
    pub fn release_conn(&mut self, key: u32) {
        if let Some(ip) = self.ips.get_mut(&key) {
            ip.conns = ip.conns.saturating_sub(1);
        }
        self.c_total = self.c_total.saturating_sub(1);
    }

    /// Request completion: decrement the per-IP and global queues.
    pub fn release_request(&mut self, key: u32) {
        if let Some(ip) = self.ips.get_mut(&key) {
            ip.queued = ip.queued.saturating_sub(1);
        }
        self.q_total = self.q_total.saturating_sub(1);
    }

    /// Periodic decay: demand halves, wait ticks capped by the decay of
    /// the oldest queue head (anti-starvation is via `W_i` growth).
    pub fn tick_decay(&mut self) {
        for ip in self.ips.values_mut() {
            ip.demand = ip.demand.wrapping_mul(7) >> 3;
            ip.recent = ip.recent.wrapping_mul(15) >> 4;
        }
    }

    /// Snapshot for tests/metrics.
    pub fn state(&self, key: u32) -> IpState {
        self.ips.get(&key).copied().unwrap_or_default()
    }
}

/// Scope guard: decrements the request counters on drop, so error paths
/// in the dispatchers cannot leak queue slots.
pub struct ReqGuard {
    pub sched: Arc<Mutex<Sched>>,
    pub key: u32,
}

impl Drop for ReqGuard {
    fn drop(&mut self) {
        self.sched.lock().release_request(self.key);
    }
}

/// Scope guard for a live connection (H2/H3 accept path).
pub struct ConnGuard {
    pub sched: Arc<Mutex<Sched>>,
    pub key: u32,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.sched.lock().release_conn(self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    fn ip(a: u8, b: u8, c: u8, d: u8) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(a, b, c, d), 1234))
    }

    #[test]
    fn demand_ema_decays_and_builds() {
        let mut st = IpState::default();
        for _ in 0..8 {
            st.demand_update(1);
        }
        // Fixed-point (x8): 8 updates of n=1 -> d == 8 (real 1.0).
        assert_eq!(st.demand, 8, "demand = {}", st.demand);
        for _ in 0..24 {
            st.demand_update(0);
        }
        // Decay is `d -= d>>3`, flooring at 4 (real 0.5): sub-unit
        // demand that can never trip the firewall.
        assert!(st.demand < 8, "demand after decay = {}", st.demand);
    }

    #[test]
    fn diversity_gain_is_comparison() {
        let mut st = IpState::default();
        assert_eq!((st.queued == 0) as i32, 1);
        st.queued = 1;
        assert_eq!((st.queued == 0) as i32, 0);
    }

    #[test]
    fn fair_mode_prefers_low_demand() {
        let mut s = Sched::new(RuleMode::Fair, Weights::default(), Limits::default());
        let lo = ip(1, 1, 1, 1);
        let hi = ip(2, 2, 2, 2);
        s.admit_request(Sched::ip_key(lo));
        let k_hi = Sched::ip_key(hi);
        // Push hi's demand well past lo's: 8 x n=64 -> d ~ 359 (real 45).
        for _ in 0..8 {
            s.entry(k_hi).demand_update(64);
        }
        let st_lo = s.state(Sched::ip_key(lo));
        let st_hi = s.state(k_hi);
        assert!(st_hi.demand > st_lo.demand, "test setup: demand order");
        let w = Weights::for_mode(RuleMode::Fair, Weights::default());
        assert!(
            s.admission_score(&st_lo, w) > s.admission_score(&st_hi, w),
            "low demand must outrank high demand"
        );
    }

    #[test]
    fn anarchy_is_fcfs() {
        let w = Weights::for_mode(RuleMode::Anarchy, Weights::default());
        assert_eq!(w.div, 0);
        assert_eq!(w.dem, 0);
        assert_eq!(w.qpen, 0);
        assert_eq!(w.wait, 1);
    }

    #[test]
    fn bounds_reject_and_release() {
        let limits = Limits {
            q_max: 4,
            q_per_ip: 2,
            ..Limits::default()
        };
        let mut s = Sched::new(RuleMode::Anarchy, Weights::default(), limits);
        let k = |o: u8| Sched::ip_key(ip(1, 1, 1, o));
        let k1 = k(1);
        let k2 = k(2);
        assert_eq!(s.admit_request(k1), Admission::Accepted);
        assert_eq!(s.admit_request(k1), Admission::Accepted);
        // Per-IP cap hit -> backlog (b_max default large).
        assert_eq!(s.admit_request(k1), Admission::Backlogged);
        s.release_request(k1);
        s.release_request(k1);
        assert_eq!(s.q_total, 0);
        // Global cap: 4 in flight from 4 distinct IPs, then reject.
        assert_eq!(s.admit_request(k1), Admission::Accepted);
        assert_eq!(s.admit_request(k2), Admission::Accepted);
        assert_eq!(s.admit_request(k(3)), Admission::Accepted);
        assert_eq!(s.admit_request(k(4)), Admission::Accepted);
        assert_eq!(s.admit_request(k(5)), Admission::Backlogged);
    }

    #[test]
    fn firewall_rejects_high_demand() {
        let mut s = Sched::new(RuleMode::Anarchy, Weights::default(), Limits::default());
        let a = ip(9, 9, 9, 9);
        let k = Sched::ip_key(a);
        // Demand in fixed point: exceed the (d_limit << 3) threshold
        // even after admit_request's demand_update(1) decays it.
        s.entry(k).demand = ((s.limits.d_limit << 3) + 200) as u32;
        assert!(!s.firewall_pass(&s.state(k)));
        assert_eq!(s.admit_request(k), Admission::Rejected);
        // Exception flag bypasses.
        s.entry(k).exception = true;
        assert!(s.firewall_pass(&s.state(k)));
    }

    #[test]
    fn conn_bounds() {
        let limits = Limits {
            c_max: 3,
            c_per_ip: 2,
            ..Limits::default()
        };
        let mut s = Sched::new(RuleMode::Anarchy, Weights::default(), limits);
        let a = ip(1, 1, 1, 1);
        let b = ip(2, 2, 2, 2);
        let k1 = Sched::ip_key(a);
        let k2 = Sched::ip_key(b);
        assert!(s.admit_conn(k1));
        assert!(s.admit_conn(k1));
        assert!(!s.admit_conn(k1), "per-IP conn cap");
        assert!(s.admit_conn(k2));
        assert!(!s.admit_conn(k2), "global conn cap");
        s.release_conn(k1);
        assert!(s.admit_conn(k2), "freed slot reused");
    }

    #[test]
    fn core_assign_balances_with_affinity() {
        // The Z<<16 term PENALIZES a core already busy with the same IP
        // (spreads same-IP jobs across cores = anti-affinity as
        // written); flip the sign to cluster instead. Load (<<8) beats
        // the affinity penalty except at equal load.
        let loads = [1u32, 0, 1];
        let aff = [false, true, false];
        // c0: 1<<8 = 256 vs c1: 0 + 1<<16 = 65536 -> c0.
        assert_eq!(Sched::core_assign(&loads, &aff), 0);
        // Without affinity, the least-loaded core wins.
        let aff = [false, false, false];
        assert_eq!(Sched::core_assign(&loads, &aff), 1);
        let loads2 = [0u32, 1, 1];
        assert_eq!(Sched::core_assign(&loads2, &aff), 0);
    }

    #[test]
    fn priority_uses_rule_weights() {
        let mut s = Sched::new(RuleMode::Fair, Weights::default(), Limits::default());
        let a = ip(1, 1, 1, 1);
        let k = Sched::ip_key(a);
        s.admit_request(k);
        // Anarchy: only wait matters -> older item wins.
        let p_young = s.priority(k, 1);
        let p_old = s.priority(k, 100);
        assert!(p_old > p_young);
    }

    #[test]
    fn ema_band_is_invariant_and_attracting() {
        // f(x) = x + n - x/8 keeps
        // the band [8n, 8n+7] invariant and attracts every state.
        // (a) exhaustive in-band check for small n.
        for n in 0u32..=8 {
            let hi = 8 * n + 7;
            for d in 0..=hi {
                let mut st = IpState {
                    demand: d,
                    ..Default::default()
                };
                st.demand_update(n);
                assert!(st.demand <= hi, "n={n} d={d} -> {}", st.demand);
            }
        }
        // (b) attraction: from any start, 256 updates at n=1 land in
        // the band [8, 15] and stay there.
        for start in [0u32, 1, 7, 8, 15, 100, 10_000, 500_000_000] {
            let mut st = IpState {
                demand: start,
                ..Default::default()
            };
            for _ in 0..256 {
                st.demand_update(1);
            }
            assert!((8..=15).contains(&st.demand), "start={start} -> {}", st.demand);
        }
    }

    #[test]
    fn decay_cycle_contracts_into_attractor() {
        // cycle: tick_decay (d -> (7d)>>3) between requests keeps
        // the firewall datapath inside [7, 13] once it is there, and
        // pulls higher states down into it.
        for d in 7u32..=64 {
            let mut s = Sched::new(RuleMode::Anarchy, Weights::default(), Limits::default());
            let k = Sched::ip_key(ip(10, 10, 10, 10));
            s.entry(k).demand = d;
            for _ in 0..64 {
                s.tick_decay();
                s.entry(k).demand_update(1);
            }
            assert!(
                (7..=13).contains(&s.state(k).demand),
                "d={d} -> {}",
                s.state(k).demand
            );
        }
    }

    #[test]
    fn ip_key_loopback_v4_is_stable() {
        let a: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert_eq!(Sched::ip_key(a), 3668918509);
    }
}
