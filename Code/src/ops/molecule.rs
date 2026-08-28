//! Named composition of atoms. Criticality C1.

use crate::atom::AtomKind;
use crate::error::AtomError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoleculeKind {
    Pure,
    Effectful,
    Hybrid,
}

pub struct Molecule {
    pub name: &'static str,
    pub kind: MoleculeKind,
    pub steps: &'static [&'static str],
}

pub const RESTART: Molecule = Molecule {
    name: "server.restart",
    kind: MoleculeKind::Effectful,
    steps: &["server.stop", "server.start"],
};

pub const OPS_DASHBOARD: Molecule = Molecule {
    name: "ops.dashboard",
    kind: MoleculeKind::Pure,
    steps: &["signal.get", "resource.get"],
};

/// Load-time check. Hybrid fails if any Effectful step appears before a later Pure.
pub fn validate_molecule(kind: MoleculeKind, step_kinds: &[AtomKind]) -> Result<(), AtomError> {
    if kind != MoleculeKind::Hybrid {
        return Ok(());
    }
    let mut seen_effect = false;
    for k in step_kinds {
        match k {
            AtomKind::Effectful => seen_effect = true,
            AtomKind::Pure => {
                if seen_effect {
                    return Err(AtomError::Input("effectful before pure".into()));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_effect_before_pure_is_err() {
        let steps = [AtomKind::Effectful, AtomKind::Pure];
        assert!(validate_molecule(MoleculeKind::Hybrid, &steps).is_err());
    }

    #[test]
    fn hybrid_pure_then_effect_is_ok() {
        let steps = [AtomKind::Pure, AtomKind::Effectful];
        assert!(validate_molecule(MoleculeKind::Hybrid, &steps).is_ok());
    }

    #[test]
    fn restart_is_effectful() {
        assert_eq!(RESTART.kind, MoleculeKind::Effectful);
    }

    #[test]
    fn ops_dashboard_is_pure() {
        assert_eq!(OPS_DASHBOARD.kind, MoleculeKind::Pure);
    }
}
