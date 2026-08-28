# GitHub-publishable Atomos Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Atomos GitHub-publishable: `Code/` and `Docs/` only as content directories, vendored FDS crates, split hot modules, ASD-STE100 docs, no em dashes, hardware layout fixes, tests and clippy green.

**Architecture:** Package root moves to `Code/`. User docs live in `Docs/`. FDS is a file copy of `mol-core` and `fds-core`. Kernel splits keep public import paths. Shared atomics take one cache line. `ip_key` does not allocate.

**Tech Stack:** Rust 1.97.1, Cargo, Linux epoll, fds-core path dep, ASD-STE100 markdown.

**Spec:** `docs/superpowers/specs/2026-08-28-github-publishable-design.md`

## Global Constraints

- ASD-STE100 for README, Docs, crate docs, and sentence comments.
- No U+2014 and no U+2013 in tracked files (except a mention of the forbidden mark in the spec, which is deleted at the end).
- Author is XENOT Corporation. No "Alex". No "@AscendNoosphere".
- Do not change JSON schemas, atom names, default bind `127.0.0.1:8090`, or HTTP results.
- Public imports used by bins/examples/tests stay (`atomos::config`, `atomos::epoll`, `atomos::serve`, ...).
- cwd for Cargo is `Code/`.
- No GitHub create or push.
- `cargo test`, `cargo test --release`, and `cargo clippy --all-targets -- -D warnings` from `Code/` must pass.
- Cache line is 64 bytes.

---

### Task 1: Vendor FDS and move the crate into Code/

**Files:**
- Create: `Code/Cargo.toml`, `Code/FDS/crates/fds-core/**`, `Code/FDS/crates/mol-core/**`
- Modify: `.gitignore`, `LICENSE`
- Move: `src`, `tests`, `examples`, `scripts`, `plugins`, `templates`, `wit`, `Cargo.lock`
- Delete: root `Cargo.toml`, tracked `.cargo/config.toml`

**Interfaces:**
- Consumes: current package manifest
- Produces: `Code/` as the Cargo package. `fds-core = { path = "FDS/crates/fds-core", default-features = false }`

- [ ] **Step 1: Record the current test baseline**

Run from repo root:

```
cargo test --lib --offline
```

Expected: PASS (or note any existing failure before touching layout).

- [ ] **Step 2: Vendor FDS crates**

```
mkdir -p Code/FDS/crates
cp -a ../FDS/crates/fds-core Code/FDS/crates/fds-core
cp -a ../FDS/crates/mol-core Code/FDS/crates/mol-core
rm -rf Code/FDS/crates/fds-core/target Code/FDS/crates/mol-core/target
```

In both vendored `Cargo.toml` files replace workspace keys:

```
version = "0.1.0"
edition = "2021"
rust-version = "1.97.1"
license = "MIT OR Apache-2.0"
```

Keep `mol-core = { path = "../mol-core" }` in fds-core.

- [ ] **Step 3: git mv sources**

```
git mv src Code/src
git mv tests Code/tests
git mv examples Code/examples
git mv scripts Code/scripts
git mv plugins Code/plugins
git mv templates Code/templates
git mv wit Code/wit
git mv Cargo.lock Code/Cargo.lock
```

- [ ] **Step 4: Write Code/Cargo.toml**

Same package as today. Drop explicit `path = "src/lib.rs"` (default). Bin paths stay `src/bin/*.rs`. Example paths stay `examples/*.rs`. Add `login_server` and `bench_h23`. Set `readme = "../README.md"`. FDS path dep as above.

- [ ] **Step 5: Root metadata**

`.gitignore`:

```
/Code/target
/Code/.cargo/config.toml
**/.atomos/
.local/
**/*.rs.bk
*.swp
.DS_Store
/keys.json
*.sock
```

LICENSE copyright line: `Copyright (c) 2026 XENOT Corporation`

Delete tracked `.cargo/config.toml` and root `Cargo.toml`.

Update `Code/scripts/*.sh` ROOT (parent of scripts is `Code/`). That is already `dirname/..`.

- [ ] **Step 6: Prove the move**

```
cd Code && cargo test --offline
```

Expected: PASS. Fix path-only breaks. Do not change assertions.

- [ ] **Step 7: Commit**

```
git add -A
git commit -m "refactor: move crate into Code/ and vendor FDS crates"
```

---

### Task 2: Hardware layout tests then fixes

**Files:**
- Modify: `Code/src/kernel/align.rs`, `Code/src/kernel/cache.rs`, `Code/src/kernel/static_mod.rs`, `Code/src/kernel/sched.rs`, `Code/src/kernel/config.rs`, `Code/src/net/epoll.rs`, `Code/src/net/h2serve.rs`

**Interfaces:**
- Consumes: `LineAtomicU64`
- Produces: line-padded epoch/hits/H2 counters; allocation-free `ip_key`; 16 MiB default cache

- [ ] **Step 1: Write failing tests**

In `sched` tests:

```rust
#[test]
fn ip_key_loopback_v4_is_stable() {
    let a: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    assert_eq!(Sched::ip_key(a), 3668918509);
}
```

In `cache` tests, after changing the field, assert:

```rust
assert_eq!(std::mem::align_of::<crate::align::LineAtomicU64>(), 64);
```

Add `static_mod` test:

```rust
assert_eq!(std::mem::align_of::<crate::align::LineAtomicU64>(), 64);
assert_eq!(std::mem::size_of::<crate::align::LineAtomicU64>(), 64);
```

- [ ] **Step 2: `ip_key` without heap**

Replace the body with:

```rust
pub fn ip_key(peer: std::net::SocketAddr) -> u32 {
    let mut h = 0x811c_9dc5u32;
    match peer {
        std::net::SocketAddr::V4(a) => {
            for b in a.ip().octets() {
                h ^= b as u32;
                h = h.wrapping_mul(0x0100_0193);
            }
        }
        std::net::SocketAddr::V6(a) => {
            for b in a.ip().octets() {
                h ^= b as u32;
                h = h.wrapping_mul(0x0100_0193);
            }
        }
    }
    h
}
```

- [ ] **Step 3: Line-pad shared atomics**

`ResponseCache.epoch: Arc<LineAtomicU64>`. Use `.v.fetch_add` / `.v.load`.

`StaticMod.hits: LineAtomicU64`. Use `.v.fetch_add`.

H2 `CountingIo` counters: `Arc<LineAtomicU64>`.

- [ ] **Step 4: RAM and pre-size**

`default_cache_bytes` -> `16 * 1024 * 1024`.

Epoll: `HashMap::with_capacity(CONN_CAP)`, `out: Vec::with_capacity(2048)`.

FdCache: `HashMap::with_capacity(FD_CACHE_MAX)`.

- [ ] **Step 5: Test**

```
cd Code && cargo test --lib --offline
```

Expected: PASS including `ip_key_loopback_v4_is_stable`.

- [ ] **Step 6: Commit**

```
git commit -m "perf: line-pad shared atomics and stop ip_key heap"
```

---

### Task 3: Split rules, sched, config

**Files:**
- Create: `Code/src/kernel/rules/{mod,parse,scan,trie}.rs`
- Create: `Code/src/kernel/sched/{mod,ip,firewall,score}.rs`
- Create: `Code/src/kernel/config/{mod,defaults,validate,host}.rs`
- Delete: `Code/src/kernel/rules.rs`, `sched.rs`, `config.rs`

**Interfaces:**
- Produces: same `Ruleset::parse`, `Sched::ip_key`, `Config::from_json`

Move existing tests with the type they cover. `kernel/mod.rs` still `pub mod rules;` (directory).

- [ ] **Step 1: Split with git mv + module files. Keep public types in `mod.rs`.**
- [ ] **Step 2:** `cd Code && cargo test --lib --offline`
- [ ] **Step 3: Commit** `refactor: split rules, sched, and config modules`

---

### Task 4: Split ctl, jail, atom

**Files:**
- Create: `Code/src/ops/ctl/{mod,cmd,keys,prompt,json}.rs`
- Create: `Code/src/ops/jail/{mod,landlock,seccomp,privs}.rs`
- Create: `Code/src/ops/atom/{mod,pure,effectful}.rs`

Public: `ctl::Env`, `ctl::run_cli`, `jail::after_bind`, `atom::dispatch`, `atom::AtomCtx`.

- [ ] **Step 1: Split.**
- [ ] **Step 2:** `cd Code && cargo test --lib --offline`
- [ ] **Step 3: Commit** `refactor: split ctl, jail, and atom modules`

---

### Task 5: Split epoll and serve

**Files:**
- Create: `Code/src/net/epoll/{mod,conn,write}.rs`
- Create: `Code/src/net/serve/{mod,accept,detect,h1}.rs`

Public: `epoll::run`, `serve::run`.

- [ ] **Step 1: Split. Keep FDS types in epoll.**
- [ ] **Step 2:** `cd Code && cargo test --offline`
- [ ] **Step 3: Commit** `refactor: split epoll and tokio serve`

---

### Task 6: Em dashes, author, dead files

**Files:** all tracked `*.rs`, `*.md`, `*.html`, `*.sh`, `*.toml`

- [ ] **Step 1: Delete** `Code/examples/first_app/rules-keyed.json`
- [ ] **Step 2: Replace U+2014 and U+2013** with `. ` or `: ` or ` (` `)`. CSS `content:"— "` becomes `content:"- "`.
- [ ] **Step 3: Remove** `Alex` and `@AscendNoosphere` from tracked files.
- [ ] **Step 4:** `python3 -c` scan of tracked files; must print none.
- [ ] **Step 5:** `cd Code && cargo test --offline && cargo clippy --all-targets -- -D warnings`
- [ ] **Step 6: Commit** `chore: strip em dashes, dead files, and personal author marks`

---

### Task 7: Docs (STE)

**Files:**
- Create: `README.md` (root, short), `Docs/Getting-Started.md`, `Docs/Maintain.md`, `Docs/Benchmarks.md`, `Docs/Wiki/*.md`
- Modify: `Code/templates/README.md`, `Code/plugins/README.md`
- Delete: `WIKI.md`, `BENCHMARKS.md`

Wiki pages: Home, Overview, Architecture, Requests, Rules, Modules, Atoms, Control, Http2-Http3, Plugins, Configuration, Performance, Examples, Limits, Binaries.

Language: short sentences, locked terms, numbered procedures, no contractions, no em dashes.

Getting Started cwd is `Code/`. FDS is already vendored.

- [ ] **Step 1: Write docs.**
- [ ] **Step 2: Every markdown link must resolve.**
- [ ] **Step 3: Commit** `docs: STE Getting Started, wiki, and benchmarks`

---

### Task 8: Final hygiene and delete agent docs

**Files:**
- Delete: `docs/superpowers/` (old path) entirely from the public tree.

- [ ] **Step 1:** Confirm root directories are `Code/` and `Docs/` plus `.git`.
- [ ] **Step 2:**

```
cd Code && cargo test --offline && cargo test --release --offline && cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 3: Scan em dashes and dead links.**
- [ ] **Step 4: Delete `docs/superpowers/`.**
- [ ] **Step 5: Commit** `chore: drop agent process docs from the public tree`

---
