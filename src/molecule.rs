//! Named composition of atoms. Criticality C1.

pub struct Molecule {
    pub name: &'static str,
    pub steps: &'static [&'static str],
}

pub const RESTART: Molecule = Molecule {
    name: "server.restart",
    steps: &["server.stop", "server.start"],
};

pub const TUI_DASHBOARD: Molecule = Molecule {
    name: "tui.dashboard",
    steps: &["signal.get", "resource.get"],
};
