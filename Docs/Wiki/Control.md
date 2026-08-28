# Control

The control socket is a Unix stream of JSON lines. Mode is 0600. Default path is under the runtime directory as `atomos.sock`.

The operator binary is `atomos-ctl`. It is an interactive prompt, or JSON lines on stdin.

Commands include: `status`, `keys list`, `keys add`, `keys del`, `json dump`, `config`, `start`, `stop`, `restart`, `refresh`, `backup`, `dry-test`, `quit`.

Destructive commands need `--yes`.

`Code/examples/data.json` is the default data file for keys CRUD.

Example:

```
cd Code
cargo run --release --bin atomos-ctl -- --config examples/first_app/config.json
echo '{"cmd":"status"}' | cargo run --release --bin atomos-ctl -- --config examples/first_app/config.json --json
```

HTTP bind and the control socket are separate. `atomos-ctl` does not bind HTTP and does not spawn a second server.
