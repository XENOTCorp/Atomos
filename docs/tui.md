# Operator TUI

Binary `atomos-ctl`. Separate process. Feature `tui` (default on).

```
atomos-ctl --config examples/config.json --json examples/data.json
atomos-ctl install-link    # $HOME/atomos → this binary
```

The TUI never binds HTTP. It:

- reads status / resources through atoms
- CRUDs JSON (`/keys` array) through `json.crud`
- sends control-socket commands for start / stop / restart / refresh

| Key | Pane | Action |
|---|---|---|
| Tab | all | next pane |
| j / k | JSON | move |
| a / d / v | JSON | add / delete (confirm) / reveal |
| r / s / t / f / b / y | Control | restart / stop / start / refresh / backup / dry-test |
| q | all | quit |

`server.start` does not spawn a binary. Run `atomos` yourself. Default listen
is **8090**, not 8082.
