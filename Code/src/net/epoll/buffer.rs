//! Bounded geometric growth for request buffers.
/// Keep both requested capacity and length within the configured wire cap.
pub(super) fn append_bounded(buf: &mut Vec<u8>, src: &[u8], max: usize) -> bool {
    let need = buf.len().saturating_add(src.len());
    if need > max {
        return false;
    }
    if src.len() > buf.capacity().saturating_sub(buf.len()) {
        let target = buf.capacity().saturating_mul(2).max(need).min(max);
        buf.reserve_exact(target - buf.len());
    }
    buf.extend_from_slice(src);
    true
}
