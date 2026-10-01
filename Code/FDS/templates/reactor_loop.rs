//! Delayed-feedback template: the register from one step feeds the next.
use mol::Molecule;

pub struct Accumulate;

impl Molecule for Accumulate {
    type State = ();
    type Input = (u64, u64);
    type Output = (u64, u64);

    fn step(&self, _: &mut (), (input, previous): Self::Input) -> Self::Output {
        let next = previous.wrapping_add(input);
        (next, next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mol::tr;

    #[test]
    fn carries_register_between_steps() {
        let reactor = tr(Accumulate);
        let mut state = ((), 0);
        assert_eq!(reactor.step(&mut state, 3), 3);
        assert_eq!(reactor.step(&mut state, 4), 7);
        assert_eq!(state.1, 7);
    }
}
