//! Effectful atom template: explicit, caller-owned runtime context.
use mol::{Atom, EffectfulAtom};

#[derive(Default)]
pub struct Context {
    pub processed: u64,
}

pub struct Count;

impl Atom for Count {
    type Input = u32;
    type Output = u32;
}

impl EffectfulAtom<Context> for Count {
    fn apply(&self, ctx: &mut Context, input: u32) -> u32 {
        ctx.processed = ctx.processed.wrapping_add(1);
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_context_without_changing_input() {
        let mut ctx = Context::default();
        assert_eq!(Count.apply(&mut ctx, 7), 7);
        assert_eq!(Count.apply(&mut ctx, 9), 9);
        assert_eq!(ctx.processed, 2);
    }
}
