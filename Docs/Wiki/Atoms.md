# Atoms

Atoms are the only mutation API. A pure atom that writes the world is a defect.

Pure atoms:

- `signal.get`
- `json.pretty`
- `resource.get`
- `rules.dry_test`

Effectful atoms:

- `json.crud`
- `settings.backup`
- `server.start`
- `server.stop`
- `server.restart`
- `rules.reload`
- `tunnel.apply`

A molecule is a named list of atoms:

```
server.restart = ["server.stop", "server.start"]
ops.dashboard  = ["signal.get", "resource.get"]
```

`allow_write: false` on the atom context turns effectful atoms into no-op actuators.

Templates: `Code/templates/atom_pure.rs`, `atom_effectful.rs`, `molecule.rs`.
