//! Template: a molecule is a **named list of atom names**.
//! It does not invent a third effect channel. Restart is stop then start.

use atomos::molecule::Molecule;

/// Built-in: `server.restart` → `["server.stop", "server.start"]`.
pub const RESTART: Molecule = Molecule {
    name: "server.restart",
    steps: &["server.stop", "server.start"],
};

/// Example: backup then dry-test the ruleset.
pub const BACKUP_THEN_DRY: Molecule = Molecule {
    name: "ops.backup_dry",
    steps: &["settings.backup", "rules.dry_test"],
};

/// Example: TUI status pane.
pub const DASHBOARD: Molecule = Molecule {
    name: "tui.dashboard",
    steps: &["signal.get", "resource.get"],
};
