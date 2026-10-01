//! Fixed-capacity byte buffers. Object-pool compatibility exports remain here;
//! pool ownership and initialization are implemented separately in `pool`.
pub use crate::pool::{Pool, PoolGuard};

#[repr(align(64))]
#[derive(Clone)]
pub struct Buffer<const N: usize> {
    data: [u8; N],
    len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetLenError;

impl<const N: usize> Buffer<N> {
    pub const fn new() -> Self {
        Self {
            data: [0; N],
            len: 0,
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data[..self.len]
    }
    pub fn as_full_slice(&self) -> &[u8; N] {
        &self.data
    }
    pub fn as_mut_full_slice(&mut self) -> &mut [u8; N] {
        &mut self.data
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub const fn capacity(&self) -> usize {
        N
    }
    pub fn set_len(&mut self, len: usize) -> Result<(), SetLenError> {
        if len > N {
            return Err(SetLenError);
        }
        self.len = len;
        Ok(())
    }
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<const N: usize> Default for Buffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buffer_bounds() {
        let mut buffer = Buffer::<16>::new();
        assert!(buffer.set_len(16).is_ok());
        assert!(buffer.set_len(17).is_err());
        buffer.as_mut_slice()[0] = 7;
        assert_eq!(buffer.as_slice()[0], 7);
        buffer.clear();
        assert_eq!(buffer.len(), 0);
    }
}
