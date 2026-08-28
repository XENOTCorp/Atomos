//! Integer admission score and priority.
use super::{IpState, Sched, Weights};

impl Sched {
    /// Admission score `A_i` (diversity + demand + exception + wait - q).
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

    /// Core assignment: `argmin(L_c << 8 + Z_{i,c} << 16)`: load
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

}
