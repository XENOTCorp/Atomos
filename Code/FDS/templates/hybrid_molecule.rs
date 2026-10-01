//! Hybrid molecule template: local state paired with runtime context.
use mol::Molecule;

#[derive(Default)]
pub struct Context {
    pub processed: u64,
}

pub struct RunningTotal;

impl Molecule for RunningTotal {
    type State = (u64, Context);
    type Input = u32;
    type Output = u64;

    fn step(&self, state: &mut Self::State, input: u32) -> u64 {
        state.0 = state.0.wrapping_add(u64::from(input));
        state.1.processed = state.1.processed.wrapping_add(1);
        state.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_local_and_context_state() {
        let mut state = (0, Context::default());
        assert_eq!(RunningTotal.step(&mut state, 3), 3);
        assert_eq!(RunningTotal.step(&mut state, 4), 7);
        assert_eq!(state.1.processed, 2);
    }
}
