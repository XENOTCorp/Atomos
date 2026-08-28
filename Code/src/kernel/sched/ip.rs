//! Per-IP integer demand state.
pub(crate) const EMA_SHIFT: u32 = 3;

#[derive(Clone, Copy, Debug, Default)]
pub struct IpState {
    /// Demand estimate (EMA, fixed point ×8: `d += n - (d>>3)`).
    pub demand: u32,
    /// Requests currently queued/running for this IP.
    pub queued: u32,
    /// Live full-duplex connections from this IP.
    pub conns: u32,
    /// Exception (whitelist) flag: bypasses the firewall.
    pub exception: bool,
    /// Wait ticks of the oldest queued item.
    pub wait_ticks: u32,
    /// Requests in the current window (1 s, rolling decay).
    pub recent: u32,
    /// Incomplete handshakes (SYN without ACK).
    pub syns: u32,
    /// Malformed requests / protocol errors.
    pub errs: u32,
    /// Set once any firewall feature crosses a low-water mark; the
    /// admission gate skips the firewall predicate entirely while this
    /// is clear (the fast/shortcut path: the firewall cannot fail).
    pub hot: bool,
}

impl IpState {
    /// `D = (7D + n) >> 3`: the integer EMA, one multiply-add+shift.
    /// `D` is stored in fixed point with 3 fractional bits (units of
    /// 1/8): a naive `(7D+n)>>3` on integer D can never leave 0
    /// (truncation), so the equivalent `D += n - (D>>3)` form is used,
    /// and demand thresholds/priotities compare against `limit << 3`.
    ///
    /// Band invariant (; machine-checked in Lean
    /// `docs/paper/lean/Scheduler.lean`): for sustained rate `n` the
    /// fixed point is `8n` and the invariant band is `[8n, 8n+7]`. The
    /// `debug_assert` machine-checks the in-band direction (an in-band
    /// state stays in-band); out-of-band states (cold start, deliberate
    /// test pokes) are outside the theorem's hypothesis and not claimed.
    #[inline]
    pub fn demand_update(&mut self, n: u32) {
        let in_band = (self.demand as u64) <= (8 * n as u64) + 7;
        self.demand = self.demand.wrapping_add(n).wrapping_sub(self.demand >> EMA_SHIFT);
        debug_assert!(
            !in_band || (self.demand as u64) <= (8 * n as u64) + 7,
            "EMA band [8n, 8n+7] is invariant under the in-band update"
        );
    }
}
