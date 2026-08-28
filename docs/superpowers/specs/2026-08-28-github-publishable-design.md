# Atomos GitHub-publishable tree

**Status:** design, approved in session  
**Date:** 2026-08-28  
**Criticality:** C2 (layout, kernel splits, docs). No new protocol.

This spec makes Atomos ready to publish on GitHub. The session does not create a remote and does not push.

## 1. Purpose

A stranger must be able to read the repository and build it from `Code/` without a sibling FDS clone.

The tree must be easy to navigate. The only content directories are `Code/` and `Docs/`.

User text must follow ASD-STE100. Do not use em dashes. Do not keep dead files.

Kernel behavior must stay the same. `cargo test` and `cargo clippy --all-targets -- -D warnings` must pass from `Code/`.

## 2. Site and non-goals

In scope:

- Move all software under `Code/`.
- Move all user documentation under `Docs/`.
- Vendor `mol-core` and `fds-core` as ordinary files.
- Split large Rust modules.
- Rewrite docs in ASD-STE100.
- Remove em dashes from the tree (docs, comments, HTML text, CSS `content`).
- Delete dead files and agent process files from the public tree.

Out of scope:

- Create or push a GitHub remote.
- Change the FDS project at `../FDS`.
- New features (WebSocket, H1 TLS, crates.io publish).
- Change JSON config schema, rules language, atom names, or HTTP results.

## 3. Locked terms

Use one term for one concept in docs and comments:

| Term | Meaning |
|---|---|
| kernel | The Atomos library. Not a framework. Not a product server. |
| engine | I/O process: H1 epoll or proto tokio. |
| module | Named `handle(&In) -> Out` function object. |
| ruleset | Disjoint JSON map from `(method, path)` to one module. |
| atom | Named mutation API. Pure or effectful. |
| molecule | Named list of atoms. |
| worker | One pinned thread that accepts and writes. |
| consumer | Program that registers modules and starts an engine. |
| control socket | Unix stream of JSON lines for the operator. |
| plugin | Manifest plus optional Wasm component. Native `.so` is refused. |
| FDS | Transport crate used by the H1 engine. |

Do not use a synonym for a locked term.

## 4. Language rules

Apply ASD-STE100 to README, `Docs/`, crate docs (`//!`), and sentence comments.

1. Use short sentences. Prefer 20 words or fewer.
2. Put one idea in one sentence.
3. Use active voice.
4. Use numbered steps for procedures.
5. Do not use contractions.
6. Do not use slang, idiom, or metaphor.
7. Do not use the em dash character (U+2014) or the en dash character (U+2013) as punctuation.
8. Use a hyphen only inside a compound identifier (`HTTP/1.1`, `rust-version`, `login_server`).
9. Use `must` for a requirement. Use `do not` for a prohibition.
10. Use present tense for facts.

HTML example pages must not use `content:"— "` or an em dash in visible text.

## 5. Repository layout

Root has metadata files only. Root has two directories: `Code/` and `Docs/`.

```
README.md
LICENSE
.gitignore
Code/
  Cargo.toml
  Cargo.lock
  src/
  tests/
  examples/
  scripts/
  plugins/
  templates/
  wit/
  FDS/
    crates/
      fds-core/
      mol-core/
Docs/
  Getting-Started.md
  Maintain.md
  Benchmarks.md
  Wiki/
    Home.md
    Overview.md
    Architecture.md
    Requests.md
    Rules.md
    Modules.md
    Atoms.md
    Control.md
    Http2-Http3.md
    Plugins.md
    Configuration.md
    Performance.md
    Examples.md
    Limits.md
    Binaries.md
```

`Code/target/` is build output. Git must ignore it.

`Code/.cargo/config.toml` is generated on the host. Git must ignore it.

`.local/` is local maintainer state. Git must ignore it.

Do not keep `src/`, `tests/`, `examples/`, `docs/`, `scripts/`, `plugins/`, `templates/`, or `wit/` at the repository root.

Do not keep `Cargo.toml` or `Cargo.lock` at the repository root.

Do not add `.github/` in this work (no remote).

Do not add `.gitmodules` (FDS is a file copy, not a submodule).

### 5.1 Root README.md

Short pointer only:

- One paragraph: Atomos is an HTTP kernel in Rust.
- Requirements: Linux, Rust 1.97.1 or later.
- Build from `Code/`.
- Link to `Docs/Getting-Started.md` and `Docs/Wiki/Home.md`.
- License: MIT. Copyright 2026 XENOT Corporation.

Do not name a person as author. The author is XENOT Corporation.

### 5.2 LICENSE

MIT License. Copyright (c) 2026 XENOT Corporation.

Match this text in README. Remove "Alex @AscendNoosphere" from the tree.

## 6. Cargo package

The package lives in `Code/`.

Users must run Cargo from `Code/`:

```
cd Code
cargo test
cargo test --release
cargo clippy --all-targets -- -D warnings
```

`Code/Cargo.toml` keeps package metadata:

- `name = "atomos"`
- `version = "0.1.0"`
- `edition = "2021"`
- `rust-version = "1.97.1"`
- `license = "MIT"`
- Do not set `package.readme` to a second wiki. Set `readme = "../README.md"`. Crates.io publish is out of scope.

Library path is the default `src/lib.rs` (crate root is `Code/`).

Keep bins:

| name | path |
|---|---|
| atomos | `src/bin/serve.rs` |
| atomos-ctl | `src/bin/ctl.rs` |
| atomos-sup | `src/bin/sup.rs` |
| atomos-proto | `src/bin/proto.rs` |
| atomos-keyd | `src/bin/keyd.rs` |

Keep examples. List each example with an explicit `[[example]]` path under `Code/examples/`, including `login_server` and `bench_h23`.

Keep features: `default = ["h1"]`, `h1`, `proto`, `wasm`.

FDS dependency:

```
fds-core = { path = "FDS/crates/fds-core", default-features = false }
```

Do not make `fds-core` or `mol-core` workspace members of Atomos.

Release profile stays: `lto = "thin"`, `codegen-units = 1`, `opt-level = 3`, `strip = true`, `incremental = false`, `panic = "abort"`.

## 7. Vendor FDS

Copy only the crates Atomos links:

- From `/home/xenot/Projects/FDS/crates/fds-core` to `Code/FDS/crates/fds-core`
- From `/home/xenot/Projects/FDS/crates/mol-core` to `Code/FDS/crates/mol-core`

Do not copy FDS docs, benches, paper, scripts, `fds-detect`, `.git`, or `target`.

Those crates use `version.workspace = true` (and edition, rust-version, license). After the copy they are not in the FDS workspace. Rewrite each vendored `Cargo.toml` with concrete values from the FDS workspace:

- `version = "0.1.0"`
- `edition = "2021"`
- `rust-version = "1.97.1"`
- `license = "MIT OR Apache-2.0"` (keep the FDS crate license; do not relicense FDS)

Keep `mol-core = { path = "../mol-core" }` inside vendored `fds-core`.

Do not change FDS control flow, types, or syscalls. Do not strip fds-core modules.

After the copy, replace em dashes in vendored comments and crate descriptions so the Atomos tree has none. That is a punctuation pass only.

Do not add a submodule. Do not add a gitignored updater script.

`Docs/Maintain.md` must tell how to copy the two crates again from a sibling FDS tree and how to rewrite workspace keys.

## 8. Public API freeze

Keep crate-root re-exports that bins, tests, examples, and templates use today, including:

`config`, `control`, `control_std`, `ctl`, `engine`, `epoll`, `error`, `flags`, `governor`, `io`, `jail`, `json_out`, `module`, `molecule`, `num`, `ops`, `rules`, `serve`, `static_router`, `status`, `sup`

`atomos::static_router` remains a function on the crate root.

Do not change atom names:

`signal.get`, `json.pretty`, `resource.get`, `rules.dry_test`, `json.crud`, `settings.backup`, `server.start`, `server.stop`, `server.restart`, `rules.reload`, `tunnel.apply`

Do not change default bind `127.0.0.1:8090`.

Do not change rules JSON: `id`, `module`, `methods`, `include`, `exclude`, `headers`.

Internal module directories may change. External import paths above must keep compiling.

## 9. Kernel splits

Move files. Then split. Behavior must match the current tests.

After the move, paths are under `Code/src/`.

### 9.1 `kernel/rules`

| File | Holds |
|---|---|
| `rules/mod.rs` | `Ruleset`, `Rule`, `RuleError`, load-time pick of scan vs trie |
| `rules/parse.rs` | JSON parse, `HeaderRule`, disjoint check |
| `rules/scan.rs` | Linear matcher |
| `rules/trie.rs` | Deterministic automaton |

Keep `TRIE_MIN_RULES = 16`, `TRIE_MAX_RULES = 64`, `TRIE_MAX_BYTES = 16 * 1024`.

Keep existing unit tests. Move them next to the realization they cover.

### 9.2 `kernel/sched`

| File | Holds |
|---|---|
| `sched/mod.rs` | `RuleMode`, `Sched`, `Admission`, `Weights`, `Limits` |
| `sched/ip.rs` | `IpState`, EMA |
| `sched/firewall.rs` | `BnnFirewall` and default predicate |
| `sched/score.rs` | Integer score |

Keep integer-only math. Keep unit tests.

### 9.3 `ops/ctl`

| File | Holds |
|---|---|
| `ctl/mod.rs` | `Env`, `exec_cmd`, `run_cli` |
| `ctl/cmd.rs` | `Cmd`, `parse_line`, `parse_words`, `parse_json` |
| `ctl/keys.rs` | keys list/add/del |
| `ctl/prompt.rs` | `run_repl`, `help_text`, human format |
| `ctl/json.rs` | JSON line API helpers |

### 9.4 `ops/jail`

| File | Holds |
|---|---|
| `jail/mod.rs` | `after_bind`, `prepare_socket_dir` |
| `jail/landlock.rs` | Landlock restrict |
| `jail/seccomp.rs` | `SECCOMP_ALLOW`, `seccomp_filter_bytes` |
| `jail/privs.rs` | drop user/group, caps, `peer_euid_ok` |

### 9.5 `kernel/config`

| File | Holds |
|---|---|
| `config/mod.rs` | `Config`, `SchedConfig` |
| `config/defaults.rs` | serde default functions |
| `config/validate.rs` | `validate` |
| `config/host.rs` | `.atomos/host.json` overlay |

### 9.6 `net/epoll`

| File | Holds |
|---|---|
| `epoll/mod.rs` | `run`, worker loop, listener token |
| `epoll/conn.rs` | `Conn`, read cursor, compact threshold |
| `epoll/write.rs` | encode scratch, `sendfile` pending |

Keep FDS types: `Reactor`, `TcpListener`, `TcpStream`, `ConnTable`, `ConnectionId`.

Public function remains `epoll::run`.

### 9.7 `ops/atom`

| File | Holds |
|---|---|
| `atom/mod.rs` | `Atom`, `AtomKind`, `AtomCtx`, `dispatch` |
| `atom/pure.rs` | `signal.get`, `json.pretty`, `resource.get`, `rules.dry_test` |
| `atom/effectful.rs` | `json.crud`, `settings.backup`, `server.*`, `rules.reload`, `tunnel.apply` |

### 9.8 `net/serve`

| File | Holds |
|---|---|
| `serve/mod.rs` | `Running`, `run`, worker threads, runtime |
| `serve/accept.rs` | accept loop |
| `serve/detect.rs` | TCP peek, TLS, H2 preface |
| `serve/h1.rs` | tokio HTTP/1.1 handler |

Public function remains `serve::run`.

### 9.9 Comments

Replace every em dash in comments with a period, colon, or parentheses.

Do not rewrite logic in a comment pass.

## 10. Dead files and hygiene

Delete from the public tree:

- `examples/first_app/rules-keyed.json` (no reference)
- `docs/superpowers/` including this spec after implementation is complete (agent process; not user docs)
- Root `WIKI.md` and `BENCHMARKS.md` after content lives in `Docs/`
- Committed host `.cargo/config.toml` (machine AVX flags)

Fix the dead link in templates README (`docs/first-web-app.md` does not exist). Point to `Docs/Wiki/Examples.md`.

Keep `examples/data.json` (ctl default data file). Document it in `Docs/Wiki/Control.md`.

Keep plugin example `plugins/example/static.json`.

`Code/scripts/cpu-rustflags.sh` must write `Code/.cargo/config.toml`. Git must ignore that file.

`Code/scripts/atomos-host.sh` stays. Update paths.

`.gitignore` at repo root:

```
/Code/target
/Code/.cargo/config.toml
.atomos/
.local/
**/*.rs.bk
*.swp
.DS_Store
/keys.json
*.sock
```

## 11. Documentation map

All pages use the language rules in section 4.

### 11.1 Docs/Getting-Started.md

First-hour path. Numbered steps:

1. Install Rust 1.97.1 or later on Linux.
2. Open a shell in `Code/`.
3. Run `cargo test`.
4. Run the login server example.
5. Run the three `curl` commands from the current README.
6. Point to the wiki Home page.
7. Point to HTTP/2 and HTTP/3 commands for `atomos-proto`.

State that FDS crates are already in `Code/FDS`. The user does not clone FDS to build Atomos.

### 11.2 Docs/Wiki/Home.md

Table of contents with one-line descriptions. No duplicate body text.

### 11.3 Wiki pages

Each page states facts. Do not copy full source files into the wiki. Show one short example per page where a procedure needs it.

| Page | Content |
|---|---|
| Overview | What the kernel does. Two engines. |
| Architecture | Four planes. Request path. Pinning. |
| Requests | `In`, `Out`, body types, cache directives. |
| Rules | Exact and prefix. exclude. overlap is an error. |
| Modules | `Module` trait. pre and post. |
| Atoms | Pure vs effectful. molecule lists. |
| Control | `atomos-ctl`, socket, commands. |
| Http2-Http3 | `atomos-proto`. TLS. measured notes with link to Benchmarks. |
| Plugins | Manifest kinds. Wasm WIT path. `.so` refused. |
| Configuration | `config.json` fields. host overlay. hard bounds. |
| Performance | Release profile. governor. link to Benchmarks. |
| Examples | login_server, first_app, static_site, echo_api, loadgen. |
| Limits | Pipelining, WebSocket, H1 TLS, streaming on H1. |
| Binaries | `atomos`, `atomos-proto`, `atomos-ctl`, `atomos-sup`, `atomos-keyd`. |

### 11.4 Docs/Benchmarks.md

Move current `BENCHMARKS.md`. Apply language rules. Keep numbers. Keep tables.

### 11.5 Docs/Maintain.md

How to copy FDS crates again. How to rewrite workspace keys. How to generate rustflags. How to run tests and clippy.

### 11.6 Code READMEs

`Code/templates/README.md` and `Code/plugins/README.md`: short tables in STE. No dead links.

## 12. Tests and verification

Move `tests/` to `Code/tests/`. Cargo default test dir applies.

Do not change assertions to hide a break.

Required commands (cwd `Code/`):

1. `cargo test`
2. `cargo test --release`
3. `cargo clippy --all-targets -- -D warnings`

Optional but required if they already pass today: integration tests in `Code/tests/*.rs` (`epoll_smoke`, `epoll_keepalive`, `http_smoke`, `http2_h3`, `json_bomb`, `rules_dry`, `smuggling`).

A split is done only when the tests that covered the old file still pass.

Search the tree for U+2014 and U+2013. The search must find none in tracked files.

Search for `Alex @AscendNoosphere`. The search must find none.

Every markdown link in `Docs/` and README must resolve to a file that exists.

## 13. Error handling

Keep `ServeError` and `AtomError`. Keep fail-closed behavior (unknown atom, overlap at load, refused `.so`, missing wasm feature).

Do not add unwrap on the request path.

## 14. Implementation order

1. Add ignore rules. Vendor FDS crates. Create `Code/Cargo.toml` with path dep. Move source, tests, examples, scripts, plugins, templates, wit. Prove `cargo test` from `Code/`.
2. Split modules one crate area at a time. Test after each area.
3. Strip em dashes and author name. Delete dead files.
4. Write `Docs/` and root README. Delete old root wiki/benchmarks.
5. Clippy clean. Link check. Character check.
6. Delete `docs/superpowers/` from the public tree (including this spec). History keeps it.

## 15. Success

The work is done when all of the following hold:

1. Root directories are only `Code/` and `Docs/` (plus `.git`).
2. `cd Code && cargo test` passes.
3. `cd Code && cargo test --release` passes.
4. `cd Code && cargo clippy --all-targets -- -D warnings` passes.
5. No tracked em dash or en dash.
6. No dead file and no dead markdown link.
7. Getting Started and the wiki exist and use locked terms.
8. FDS is a vendored copy of two crates, not a submodule.
9. No GitHub remote was required.
