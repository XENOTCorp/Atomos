# Operator ctl

Binary `atomos-ctl`. Separate process. CLI / JSON only (no TUI). Default is
a `>` prompt, argv commands, or JSON-lines on stdin.

```
atomos-ctl --config examples/config.json --data examples/data.json
atomos-ctl --json status
atomos-ctl install-link    # $HOME/atomos → this binary
```

Ctl never binds HTTP. It:

- reads status through the Unix control socket (atoms)
- CRUDs JSON (`/keys` array) through `json.crud`
- sends `start` / `stop` / `restart` / `refresh-endpoints` / `backup` / `dry-test`

Permission denied on the socket or data file is a named `permission_denied`
error (mode 0600, same EUID). Missing socket is `server_unreachable` — that
is not the HTTP bind.

`server.start` does not spawn a binary. Run `atomos` / `first_app` yourself.
Default listen is **127.0.0.1:8090**. Extra ports to skip are `refuse_ports`
in config or `.atomos/host.json` (from `scripts/atomos-host.sh`).

Full command list and the JSON protocol: [first-web-app.md](first-web-app.md)
section 12.
