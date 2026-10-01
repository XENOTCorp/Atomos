//! Pure atom template: a total function with no runtime state.
use mol::{Atom, PureAtom};

pub struct Increment;

impl Atom for Increment {
    type Input = u32;
    type Output = u32;
}

impl PureAtom for Increment {
    fn apply(&self, input: u32) -> u32 {
        input.wrapping_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increments_and_wraps() {
        assert_eq!(Increment.apply(41), 42);
        assert_eq!(Increment.apply(u32::MAX), 0);
    }
}
