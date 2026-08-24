//! itoa/dtoa into caller buffers. Hot path must not `format!` integers.
//! Domain: finite integers/floats. Bound: dest must be ≥ itoa max (20).

pub fn u16_to_slice(n: u16, dest: &mut [u8]) -> usize {
    let mut buf = itoa::Buffer::new();
    let s = buf.format(n);
    let n = s.len();
    dest[..n].copy_from_slice(s.as_bytes());
    n
}

pub fn u64_to_slice(n: u64, dest: &mut [u8]) -> usize {
    let mut buf = itoa::Buffer::new();
    let s = buf.format(n);
    let n = s.len();
    dest[..n].copy_from_slice(s.as_bytes());
    n
}

pub fn usize_to_slice(n: usize, dest: &mut [u8]) -> usize {
    let mut buf = itoa::Buffer::new();
    let s = buf.format(n);
    let n = s.len();
    dest[..n].copy_from_slice(s.as_bytes());
    n
}

pub fn f64_to_slice(n: f64, dest: &mut [u8]) -> usize {
    let mut buf = dtoa::Buffer::new();
    let s = buf.format(n);
    let n = s.len();
    dest[..n].copy_from_slice(s.as_bytes());
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn itoa_writes_413() {
        let mut b = [0u8; 16];
        let n = u16_to_slice(413, &mut b);
        assert_eq!(&b[..n], b"413");
    }

    #[test]
    fn u64_zero() {
        let mut b = [0u8; 32];
        let n = u64_to_slice(0, &mut b);
        assert_eq!(&b[..n], b"0");
    }
}
