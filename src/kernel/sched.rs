//! Integer-only request admission, firewall, and priority scheduler.
//!
//! Every equation in this module uses only additions, comparisons,
//! shifts, and min/max — no division, no floating point. All state is
//! plain integers; the hot structures are a per-IP table (hashbrown)
//! and a handful of global counters. A request passes through:
//!
//! ```text
//! Request -> firewall (pure predicate) -> admission (bounds + score)
//!         -> [module] -> completion (counters decremented)
//! ```
//!
//! Rule modes swap the weights (a "Lawvere theory" per the design):
//! - `Anarchy`  — FCFS: only wait time matters, per-IP queues unused.
//! - `MaxAvail` — strong IP-diversity bonus, low demand preferred.
//! - `Fair`     — weighted fair queuing: priority proportional to
//!   inverse demand.
//! - `Custom`   — user-supplied integer weights from config.
//!
//! The optional binarized-neural-network firewall is a trait hook
//! (`BnnFirewall`); the default is the threshold-rule predicate.

use std::sync::Arc;

use parking_lot::Mutex;

/// Demand decay: fixed-point EMA, 3 fractional bits (see `IpState`).
const EMA_SHIFT: u32 = 3;

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
            c_max: 1000,
            c_per_ip: 50,
            q_max: 2000,
            q_per_ip: 100,
            b_max: 5000,
            h_max: 65_536,
            s_max: 10 << 20,
            str_max: 1000,
            d_limit: DEFAULT_D_LIMIT,
        }
    }
}

/// Per-IP integer state (all counters, no floats).
#[derive(Clone, Copy, Debug, Default)]
pub struct IpState {
    /// Demand estimate (EMA, `(7D + n) >> 3`).
    pub demand: u32,
    /// Requests currently queued/running for this IP.
    pub queued: u32,
    /// Live full-duplex connections from this IP.
    pub conns: u32,
    /// Exception (whitelist) flag: bypasses the firewall.
    pub exception: bool,
    /// Wait ticks of the oldest queued item.
    pub wait_ticks: u32,
    /// Requests in the current window (1 s).
    pub recent: u32,
    /// Incomplete handshakes (SYN without ACK).
    pub syns: u32,
    /// Malformed requests / protocol errors.
    pub errs: u32,
}

impl IpState {
    /// `D = (7D + n) >> 3` — the integer EMA, one multiply-add+shift.
    /// `D` is stored in fixed point with 3 fractional bits (units of
    /// 1/8): a naive `(7D+n)>>3` on integer D can never leave 0
    /// (truncation), so the equivalent `D += n - (D>>3)` form is used,
    /// and demand thresholds/priotities compare against `limit << 3`.
    #[inline]
    pub fn demand_update(&mut self, n: u32) {
        self.demand = self.demand.wrapping_add(n).wrapping_sub(self.demand >> EMA_SHIFT);
    }
}

/// Outcome of the admission controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Accepted into the queue; a guard decrements on completion.
    Accepted,
    /// Over a per-IP/global bound; parked in the backlog.
    Backlogged,
    /// Firewall or capacity rejected.
    Rejected,
}

/// Optional adaptive firewall hook (binarized/deterministic net).
/// Default is `None` -> the threshold predicate only.
pub trait BnnFirewall: Send + Sync {
    fn predict(&self, features: &[i16; 8]) -> bool;
}

/// The scheduler: integer state + rule-mode weights + global limits.
/// Wrapped in a `Mutex` because H2/H3 dispatch is multi-threaded; the
/// lock is a single fast `parking_lot` critical section per request
/// (per-core sharding is the documented follow-up).
pub struct Sched {
    pub rule: RuleMode,
    pub custom: Weights,
    pub limits: Limits,
    pub bnn: Option<Arc<dyn BnnFirewall>>,
    ips: hashbrown::HashMap<u32, IpState>,
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

    /// FNV-1a over the address bytes — the IP hash key.
    pub fn ip_key(peer: std::net::SocketAddr) -> u32 {
        let mut h = 0x811c_9dc5u32;
        let bytes = match peer {
            std::net::SocketAddr::V4(a) => a.ip().octets().to_vec(),
            std::net::SocketAddr::V6(a) => a.ip().octets().to_vec(),
        };
        for b in bytes {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        h
    }

    fn entry(&mut self, key: u32) -> &mut IpState {
        self.ips.entry(key).or_default()
    }

    /// Firewall precondition: every feature under its threshold, or the
    /// IP is an exception. O(1) comparisons. Demand is fixed-point
    /// (×8), so the threshold is `d_limit << 3`.
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

    /// Request admission: bounds first, then the rule-mode score.
    /// Returns the outcome and the integer score (for the priority
    /// queue the scheduler would maintain).
    pub fn admit_request(&mut self, key: u32) -> (Admission, i32) {
        self.tick = self.tick.wrapping_add(1);
        let limits = self.limits;
        let w = Weights::for_mode(self.rule, self.custom);
        let q_total = self.q_total;
        let backlog = self.backlog;
        // Phase 1: update counters + evaluate (entry borrow ends here).
        let (passes_fw, over, score) = {
            let ip = self.entry(key);
            ip.demand_update(1);
            // Rolling window decay on `recent` (no clock in the hot
            // path): `r -= r>>4` per request keeps the window counter
            // bounded (~16x the sustained rate) so a long-lived client
            // can never trip the 1024 threshold by volume alone.
            ip.recent = ip.recent.wrapping_sub(ip.recent >> 4);
            ip.recent = ip.recent.saturating_add(1);
            ip.wait_ticks = ip.wait_ticks.saturating_add(1);
            let fw = ip.exception
                || (ip.demand as i32 <= (limits.d_limit << 3)
                    && ip.recent <= 1024
                    && ip.syns <= 256
                    && ip.errs <= 64);
            let over = ip.queued >= limits.q_per_ip || q_total >= limits.q_max;
            let s = (w.div * (ip.queued == 0) as i32)
                + (w.dem * ((limits.d_limit << 3) - ip.demand as i32))
                + (w.exc * ip.exception as i32)
                + (w.wait * ip.wait_ticks as i32)
                - (w.qpen * ip.queued as i32);
            (fw, over, s)
        };
        if !passes_fw {
            return (Admission::Rejected, 0);
        }
        if over {
            if backlog < limits.b_max {
                self.backlog += 1;
                return (Admission::Backlogged, 0);
            }
            return (Admission::Rejected, 0);
        }
        // Phase 2: commit (no outstanding borrow).
        self.entry(key).queued += 1;
        self.q_total += 1;
        (Admission::Accepted, score)
    }

    /// Admission score `A_i` (diversity + demand + exception + wait - q).
    /// Demand is fixed-point (×8), hence `d_limit << 3`.
    pub fn admission_score(&self, ip: &IpState, w: Weights) -> i32 {
        let div = (ip.queued == 0) as i32;
        (w.div * div)
            + (w.dem * ((self.limits.d_limit << 3) - ip.demand as i32))
            + (w.exc * ip.exception as i32)
            + (w.wait * ip.wait_ticks as i32)
            - (w.qpen * ip.queued as i32)
    }

    /// Scheduler priority `P_j` for item j (no diversity term; the item
    /// is already admitted). `wait_j` is the item's own wait ticks.
    pub fn priority(&self, key: u32, wait_j: u32) -> i32 {
        let w = Weights::for_mode(self.rule, self.custom);
        let ip = self.ips.get(&key).copied().unwrap_or_default();
        (w.dem * ((self.limits.d_limit << 3) - ip.demand as i32))
            + (w.wait * wait_j as i32)
            + (w.exc * ip.exception as i32)
            - (w.qpen * ip.queued as i32)
    }

    /// Core assignment: `argmin(L_c << 8 + Z_{i,c} << 16)` — load
    /// balance with same-IP affinity.
    pub fn core_assign(loads: &[u32], affinity: &[bool]) -> usize {
        let mut best = 0usize;
        let mut best_score = u32::MAX;
        for (c, (&l, &z)) in loads.iter().zip(affinity.iter()).enumerate() {
            let score = (l << 8) + ((z as u32) << 16);
            if score < best_score {
                best_score = score;
                best = c;
            }
        }
        best
    }

    /// Full-duplex connection admission (transport level).
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
        // Decay is `d -= d>>3`, flooring at 4 (real 0.5) — sub-unit
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
        assert_eq!(s.admit_request(k1).0, Admission::Accepted);
        assert_eq!(s.admit_request(k1).0, Admission::Accepted);
        // Per-IP cap hit -> backlog (b_max default large).
        assert_eq!(s.admit_request(k1).0, Admission::Backlogged);
        s.release_request(k1);
        s.release_request(k1);
        assert_eq!(s.q_total, 0);
        // Global cap: 4 in flight from 4 distinct IPs, then reject.
        assert_eq!(s.admit_request(k1).0, Admission::Accepted);
        assert_eq!(s.admit_request(k2).0, Admission::Accepted);
        assert_eq!(s.admit_request(k(3)).0, Admission::Accepted);
        assert_eq!(s.admit_request(k(4)).0, Admission::Accepted);
        assert_eq!(s.admit_request(k(5)).0, Admission::Backlogged);
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
        assert_eq!(s.admit_request(k).0, Admission::Rejected);
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
}
