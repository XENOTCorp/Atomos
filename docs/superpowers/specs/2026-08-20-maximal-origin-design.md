# Maximal origin occupancy (strict, ARCSS)

**Status:** design, not yet implemented  
**Criticality:** C2 (kernel, net, ops, plugin). C3 not claimed.  
**Date:** 2026-08-20  
**Site:** Atomos as a loopback kernel-TCP HTTP origin behind a separate terminator (cloudflared). Default bind `127.0.0.1:8090`. Never `8082`. Never overwrite `/usr/local/bin/paper-retrieval`.

This spec answers one question: **what is the unique ARCSS-admissible inhabitant of every axis that is actually an object of this site**, and what must be forfeited because it lives in a different slice (proxy, userspace NIC, C TCB, ambient plugin VM).

No implementation heuristics. Every choice is either:

1. the unique morphism given by a universal property, an ARCSS policy, or a measured H-04 matrix, or
2. a **slice-forfeit** (the competitor object is not in the site), not a loss.

`.so` / Lua are admitted only if the H-04 gain theorem says the residual is large relative to the syscall floor **and** ARCSS still holds. It does not. The “smaller monolith” we do accept is a **TCB coproduct of binaries**, not an ambient VM.

---

## 1. Purpose

Occupy **100% of the axes of site S** (defined below). On the mixed 59-axis competitor board, that is **~80% have / ~85% have+partial / ~15% domain-correct forfeits**. Those forfeits are type errors, not unfinished work.

Keep: modularity of four planes, hot-swap of rules and modules, memory safety, loopback-first jail, measured H1 speed, affine Rust mapping.

Do not: mix concurrency models in one process (CC-00), share a mutex cache across workers (CC-02), put plugins on the cache-hit path (H-07), enlarge the TCB with C parsers or extra TLS stacks (L-02), or turn Atomos into Pingora.

---

## 2. Site slice S

Let **Web** be the (informal) category of “HTTP-ish programs” whose objects include reverse proxies, origin kernels, userspace TCP stacks, Lua/nginx module hosts, DPDK NICs, and planet anycast edges.

**S** is the full subcategory of objects **X** such that all of the following hold:

| Predicate | Meaning |
|---|---|
| `Origin(X)` | Terminates HTTP; does not pool upstreams, retry, or ketama. |
| `LoopbackTCP(X)` | Kernel TCP on `127.0.0.1`; a terminator may wrap TLS for the public Internet. |
| `Affine(X)` | ARCSS operational mapping (Rust ownership). |
| `CC00(X)` | One concurrency model per process. |
| `CC01(X)` | Parallelism (one worker process or pinned thread per core) before intra-core concurrency. |
| `CC02(X)` | Message passing / immutables / atomics; no mutex shared-memory cache. |
| `P01(X)` | `handle: In → Out` is pure; `Log` / `Actuate` live in effect adapters. |
| `H07(X)` | Cache-hit fast path is a pure predicate; plugins are not in its image. |
| `L02(X)` | Named TCB; no extra C TLS or C parser unless a Level-II waiver with bounds. |
| `Bind(X)` | Default `127.0.0.1:8090`; `refuse_ports` from host facts; never 8082 in-tree. |
| `ISA(X)` | Broadwell AVX2; no AVX-512, no DPDK/XDP on loopback. |

An **axis** is a functor `Q: S → Poset` (latency, isolation, TCB size, hot-swap, …).  
**Winning** an axis is inhabiting the unique (up to unique isomorphism) representative of `Q` in `S`.  
**Forfeiting** an axis is noticing that the competitor’s object is not in `S`. Comparing scores across a slice functor is a type error.

The old 59-board mixed `S` with `Web \ S`. This spec never recategorizes a forfeit as a gap.

---

## 3. HTTP as a theory; engines as representations

### 3.1 Theory T_HTTP

Objects: `Wire₁`, `Wire₂`, `Wire₃`, `In`, `Out`, `Err`.

Generators:

```
parse_i : Wire_i → In + Err     (i ∈ {1,2,3})
handle  : In → Out              (module; pure)
encode_i: Out → Wire_i
```

HTTP/1.1, HTTP/2, HTTP/3 are **representations** (algebras) of the same theory, not three products. The semantic kernel `handle` is the unique part that is **natural** in the representation: a natural transformation between the H1 and H2 functors is exactly “same `In`/`Out`, different wire.”

That is why `kernel/` must not import sockets, h2, or quinn, and why `net/` may.

### 3.2 Irreducibility (CC-00)

A **concurrency model** is a choice of primitives (run-to-completion epoll; cooperative tokio tasks; …). CC-00: all concurrent constructs in a process are instances of **one** declared model. Mixing is forbidden unless Level-III verified.

Analogy (not a theorem we prove): an irreducible representation has a trivial intertwiner ring. A tokio `spawn` inside an epoll worker is a non-natural endomorphism of the epoll representation. The unique ARCSS morphism that admits both models is the **coproduct in Proc** (two processes), not the product in one address space.

```
Proc  ∋  atomos-h1  ⊔  atomos-proto  ⊔  atomos-sup  ⊔  atomos-keyd
```

Config may select one `EngineKind` at process start. Linking both engines into one binary is a TCB issue (L-02), not automatically a CC-00 violation **if only one model runs**. The unique L-02 inhabitant is still **two binaries** so the H1 TCB does not contain quinn/h2/h3/rustls.

### 3.3 Declared models

| Process | Concurrency model (glossary) |
|---|---|
| `atomos` (H1) | One epoll set per pinned OS thread. Connection = slot in a map. No task spawn. Stop via atomic + `epoll_wait` timeout. |
| `atomos-proto` | Pinned current-thread tokio. H2/H3/TLS only. Cache-hit writes on the connection task. |
| `atomos-sup` | Single-threaded wait/restart; signals; no HTTP. |
| `atomos-keyd` | Single-threaded request/response on a Unix socket; no HTTP. |
| `atomos-ctl` | One-shot client; no server loop. |

`architecture.md` shall name these five and forbid a sixth model.

---

## 4. Rules, cache, plugins: unique morphisms

### 4.1 Rules = coproduct of disjoint guards

A ruleset is a finite coproduct

```
R = ∐ᵢ (Pᵢ × Mᵢ)
```

where `Pᵢ: In → Bool` are **pairwise disjoint** (overlapping include/exclude is a parse error; illegal states unrepresentable, MC-01), and `Mᵢ` is a module name. Matching is the unique mediating morphism out of the coproduct, or the zero morphism (404). Regex routers are a different algebra (backtracking, unbounded CF-02). They are not objects of S.

### 4.2 Cache = memoization of a pure morphism (H-07)

Let `C ⊂ In` be the full subcategory of cacheable requests (`GET`/`HEAD` + `CacheDirective ≠ No`). The cache is the table for `handle|C`. Selection predicate:

```
hit(k) ⇔  key_eq(k) ∧ ttl_ok ∧ epoch_ok ∧ named_epoch_ok
```

is pure and bounded. On `hit`, `handle` is not invoked (DR-04 compute-trivializing enrichment: the wire bytes are the table). Plugins, Wasm, pre, post, and atoms are **not in the image of the fast path**.

Per-worker thread-local maps + process-wide epoch atomics are the unique CC-02 representative. nginx `shared:SSL` / slab cache is mutex shared memory: **forfeit**. Duplicate RAM across cores is intended; RSS bound is `workers × cache_bytes` (explicit, R).

### 4.3 Plugins = Kleisli morphisms of a capability monad

```
Plugin : In → Cap(Out)
```

`Cap` is the monad of **granted** effects (CPU + fuel + epoch interrupt; no FS unless a future cap is added to WIT). The host is the algebra of `Cap`.

- **Wasm component + WIT** (`wit/atomos-module.wit`) is a faithful representation of `Cap`: only exported `handle` exists.
- **`.so` / Lua / mruby** are the identity monad with **ambient** IO. They are not algebras of `Cap`. They explode L-02 TCB, violate P-01 (effects inside “modules”), and cannot honour H-07 without a second, informal sandbox.

Hot-swap of **rules** is `arc-swap` of a pure coproduct (already have). Hot-swap of **native Rust modules** is `Router::insert` (already have). Hot-swap of **untrusted code** is Wasm, never `.so`.

---

## 5. Gain theorem: `.so` / Lua are not huge

H-04 requires a decision matrix, not taste. Measured on this Broadwell (loopback, pinned workers, cacheable static/health):

| Quantity | Symbol | Value |
|---|---|---|
| Serial H1 p50 (C ping / Atomos) | `T_sys` | ~30 µs / ~29 µs |
| Kernel+parse+cache floor (pipelined) | `T_sys` | ≳ 18 µs |
| Cache-hit plugin work | | **0** (H-07) |
| Dominant hot-path case | `p_hit` | → 1 on static/health |

Let `T_n` be uncached native extra, `T_w` uncached Wasm extra.

```
Δ     = (1 − p_hit) · (T_w − T_n)
gain  = Δ / T_sys
```

As `p_hit → 1`, `gain → 0`. The common case never enters a plugin. Uncached POST (notes) is in-process Rust today; `.so` would save call overhead ≪ `T_sys` and cost ambient authority.

**H-07 forbids** putting `.so` on the fast path to “make the gain large.” That would be specializing by leaking ambient effects: the Wrong Example in H-07.

**Verdict (unique):** refuse `kind: native` and Lua/mruby. Link wasmtime **off** the cache-hit path, with fuel and epoch interrupt, when untrusted hot-swap is required.

The user-offered “smaller monolith” is realized as **feature-gated binaries** (next section), which **does** shrink TCB and **does** preserve ARCSS. That gain is large on the security/modularity axes and is not a runtime heuristic.

---

## 6. Smaller monolith = TCB coproduct (accepted)

| Binary | Links | Does not link |
|---|---|---|
| `atomos` | kernel, epoll H1, ctl server (std), jail | h2, h3, quinn, rustls, wasmtime |
| `atomos-proto` | kernel, tokio, rustls, h2, h3, quinn | epoll H1 worker (optional to link, never **run** with H1 model) |
| `atomos-sup` | ops/sup | datapath |
| `atomos-keyd` | rustls keys, UDS | HTTP |
| `atomos-ctl` | ops/ctl client | HTTP server |

Cargo features:

```
default = ["h1"]
h1      = []                    # epoll engine
proto   = ["h2", "h3", "quinn", "rustls", …]
wasm    = ["wasmtime"]          # never default
```

`AtomCtx` holds **atomics only** (`signal`, `stop`, rules `ArcSwap`). `tokio::sync::Notify` lives in `net/serve.rs` under `proto`, not in `ops/atom.rs`. Kernel plane has no tokio, no sockets.

Default `Config.engine` becomes `"epoll"`. `engine=epoll` ∧ (`http2` ∨ `http3`) is a **config error** (illegal state unrepresentable). `atomos-proto` defaults `engine=tokio`, `http2=true`, `http3=true`.

This is the unique L-02 + CC-00 inhabitant. It is smaller than “one binary that is nginx+h2o+wasm+lua”.

---

## 7. Unique representative per axis

Proof sketch column: the policy or universal property that makes the choice unique in S. “H-04” means inhabit only if a measured matrix says the residual exceeds `T_sys` enough to matter; otherwise the current inhabitant **is** maximal.

### 7.1 Inhabit (closable or already have)

| Axis | Unique inhabitant in S | Why unique |
|---|---|---|
| H1 engine | Epoll RTC, Conn=slot, one loop/core, one `write` of encoded `Arc<Bytes>` on hit | CC-00 + CC-01 + H-07 |
| H2/H3/TLS | Separate **process** `atomos-proto` | CC-00 coproduct |
| H1 TCB | No quinn/h2/h3/rustls in `atomos` | L-02 |
| Parse H1 | httparse borrow-buf | Same idea as pico; C SIMD is L-02 unless a Rust port **and** H-04 wins |
| Cache | Thread-local + epoch + named epoch + full keys | CC-02 |
| Rules hot-swap | `arc-swap` coproduct | MC-01 |
| Native module hot-swap | `Router::insert` | Kleisli of identity-in-process |
| Untrusted module | Wasm component + fuel + epoch; **never on hit** | Cap monad; H-07 |
| Process isolation | `atomos-sup` one child/core, `panic=abort` per worker | CC-01 |
| Two-generation reload | SIGHUP: spawn gen N+1, drain gen N | Message to processes, not mutex |
| Drain | `worker_shutdown_timeout` (config, default 2s) | Bound (R, T) |
| Heartbeat | Worker → sup on UDS, bounded | CC-02 |
| TLS keys | `atomos-keyd` process, UDS sign | CC-02 + SEC |
| Tickets | Per-worker rustls; key set `ArcSwap`; rotation | CC-02 (not shm) |
| OCSP | File/atom staple; **not** fetched on GET | P-01 |
| Jail | `NO_NEW_PRIVS`, setuid, **Landlock**, **seccomp allowlist**, **capset** after bind | SEC |
| Ctl | `$XDG_RUNTIME_DIR`, 0600, `SO_PEERCRED` | have |
| Metrics | Atom or `/metrics` module reading atomics; not on hit | P-01 |
| Access log | `Log` combinator **after** encode; bounded | P-01, operational effect algebra |
| Fuzz / smuggling | cargo-fuzz + CL/TE tests | V-01 domain |
| CPU pin / REUSEPORT / host `/proc` | have | H-04 already |
| Default bind / refuse_ports | have | site |

### 7.2 H-04 gated (not heuristics; inhabit iff matrix)

| Candidate | Gate |
|---|---|
| Rust SIMD parse (pico-class PCMPESTRI/AVX2) | Must beat httparse on this ISA without C TCB |
| `writev` / `sendfile` | Only for large uncached bodies; cache hit is already one buffer |
| `EPOLLEXCLUSIVE` | Only if thundering-herd is measured with REUSEPORT |
| `recvmmsg` | UDP; not H1 TCP origin |
| Prefetch, io_uring, simd-json | **Already measured; rejected.** Do not reopen without a new matrix. |

### 7.3 Slice-forfeits (not objects of S)

| Competitor object | Why not in S |
|---|---|
| nginx slab / `shared:SSL` | CC-02 mutex shm |
| Cache manager / loader processes | No disk cache in S; YAGNI until a persistent-cache module exists |
| kTLS | Kernel OpenSSL TCB; rustls is the unique L-02 TLS |
| FIPS / BoringSSL / s2n **as extra stacks** | Multi-TCB; rustls+ring only |
| Userspace TCP (F-Stack, Seastar native) | Not loopback kernel-TCP; ATC 2025: no stack wins all sizes |
| AF_XDP / DPDK on this host | Needs a NIC; loopback cannot; `EngineKind::Xdp` stays an unlinked slot for a **different** site |
| HTTP/2 as the **fast** path on this box | H-04: H1 ~143k > h2 ~39k > h3 ~5.3k. Fast path is H1. Proto still **inhabits** H2/H3 correctness. |
| H3 at C quicly class on Broadwell | Silicon + C TCB; we inhabit quinn/rustls in proto |
| Lua / mruby / nginx `load_module` `.so` | Ambient Cap; gain theorem |
| Pingora upstream pool, retry, ketama, gRPC proxy | `Origin(X)` fails: different category (consumer/proxy) |
| Planet 40M rps anycast | Hardware + anycast; n/a |
| picohttpparser **C** as TCB | L-02; optional Rust SIMD is §7.2 |

These are **domain-correct zeros**, not a to-do list.

---

## 8. Projected occupancy

### 8.1 Slice S (the only score that is a type-correct “win all”)

Winnable axes of S = 59-board minus the 9 forfeits in §7.3 that were counted as in-scope origin axes (shm cache, cache manager, kTLS-as-required, recvmmsg-as-required, userspace TCP, quicly-class H3, H2-as-fast-path, extra TLS stacks, XDP-on-loopback). Pico-C is not a separate forfeit if httparse is the inhabitant.

| | Count |
|---|---:|
| Winnable in S | 50 |
| Already have | 24 |
| To inhabit (this program of work) | 26 |
| **Projected have of S** | **50 / 50 = 100%** |

### 8.2 Mixed 59-board (continuity with `docs/scorecard.md`)

Today: 24 have (41%), 2 partial (3%), 4 slot (7%), 29 gap (49%).

After this spec’s program, treating bundled “writev/kTLS/zerocopy” as **partial** (writev/sendfile H-04, kTLS forfeit):

| Bucket | Count | Share |
|---|---:|---:|
| have | 47 | **80%** |
| partial (kTLS bundle, optional SIMD) | 3 | **5%** |
| forfeit (not in S) | 9 | **15%** |
| have+partial | 50 | **85%** |

Rounding: 47/59 = 79.7% → **80% have**; 50/59 = 84.7% → **~85% have+partial**; 9/59 = 15.3% → **~15% forfeit**.

**You cannot reach 100% of the 59-board without leaving S** (breaking CC-00, CC-02, L-02, or Origin). That would not be “winning all”; it would be changing the type.

**You can reach 100% of S.** That is “win on every axis possible.”

### 8.3 What the 26 closable items are

From today’s partial + slot + gap, minus forfeits:

1. SIGHUP two-generation reload  
2. `worker_shutdown_timeout`  
3. Process workers as the isolation story (finish: gen+heartbeat; crash ≠ all cores)  
4. Wasmtime host + fuel + epoch  
5. Landlock  
6. seccomp allowlist  
7. capset beyond setuid  
8. optional chroot/user ns after bind (public bind only; loopback may skip)  
9. neverbleed-style `atomos-keyd`  
10. OCSP staple from file  
11. ticket rotation (`ArcSwap`)  
12. per-worker TLS session/tickets (not shm)  
13. H2 codec on proto worker / connection task (no spawn on cache hit)  
14. stream priority / flow-control knobs in proto (config, bounded)  
15. ALPN/protocol registry as a coproduct in proto (illegal mix with epoll)  
16. spawn-per-conn **removed from H1** (epoll default; tokio not used for H1)  
17. tokio path no longer the H1 engine (partial → n/a)  
18. Prometheus-style metrics atom  
19. access log effect adapter  
20. worker heartbeat  
21. fuzz harness  
22. request-smuggling tests  
23. privilege-separated config parse (sup parses, workers inherit)  
24. `writev`/`sendfile` only if H-04  
25. Rust SIMD parse only if H-04  
26. proto binary + feature split (TCB) + `Notify` out of `AtomCtx`

XDP remains an unlinked **slot for another site**, not a gap in S.

---

## 9. Invariants I (preserved by every morphism)

```
I_bind     bind default 127.0.0.1:8090; refuse_ports honoured; never 8082 in src/examples Rust
I_engine   one EngineKind runs in a process; epoll ⇏ http2/http3
I_hit      cache hit ⇒ no module, no plugin, no Wasm, no atom, no log-before-encode
I_get      GET/HEAD never takes the POST mutex
I_cache    keys are method+path+query, not u64; epoch/named epoch monotonic
I_cc00     no tokio spawn in epoll worker; no epoll in proto worker
I_cc02     no mutex shared cache/TLS slab
I_cap      Wasm has only WIT handle + fuel/epoch; .so refused
I_tcb      atomos (H1) does not link quinn/h2/h3/rustls/wasmtime
I_jail     after_bind: NO_NEW_PRIVS; optional Landlock/seccomp/capset; ctl PEERCRED
I_rss      RSS cap from config; hard → 503; workers × cache_bytes documented
I_pure     handle: In → Out has empty effect footprint; Log after encode
```

Tests must name the invariant they protect.

---

## 10. Global constraints (normative for the plan)

- ARCSS C2; `#![deny(warnings)]`; clippy `-D warnings`.
- `CARGO_TARGET_DIR=$HOME/.cache/atomos-target`; `/tmp` is noexec; unset host `RUSTFLAGS` if they inject clang/mold.
- `panic=abort`; lld+RELRO as today.
- CPU flags from `scripts/cpu-rustflags.sh` / `scripts/atomos-host.sh` (`/proc`), never hardcoded ISA.
- No regex routers, no io_uring in product, no software prefetch, no simd-json (measured).
- Do not restart `~/boot/35-papers`; do not bind 8082; do not `atomos-ctl install-link` unless asked.
- Tests use `127.0.0.1:0` or an ephemeral port.
- Four planes remain: `kernel/` `net/` `ops/` `plugin/`. New files go in the plane that owns the morphism.

---

## 11. Approaches considered (and the unique one)

**A. One process, mixed tokio+epoll+Wasm (status quo plus more).**  
Fails CC-00 as soon as both engines run; L-02 TCB of H1 includes QUIC.

**B. `.so` / Lua in-process for “speed” and a smaller extension story.**  
Fails Cap faithfulness, H-07, L-02. Gain theorem: not huge.

**C. Coproduct of processes + Wasm off the fast path + rustls-only + thread-local cache. (chosen)**  
Unique inhabitant of S. Smaller H1 monolith. Wins every axis that is an object of S.

No fourth option that simultaneously satisfies CC-00, H-07, L-02, and Origin.

---

## 12. Data flow (H1 process)

```
accept → parse_1 → hit? ──yes──→ write(wire) → [Log]
                 └──no──→ pre → match(R) → handle → post → put cache → encode → write → [Log]
```

`handle` is pure. `Log` is the effect combinator with footprint `{log_fd}`. `par` is forbidden with overlapping footprints (operational effect algebra). Metrics scrape is a separate `handle` on a disjoint rule, or an atom on ctl: never mixed into the hit path.

---

## 13. Error handling

Config errors (mixed engine+h2, `.so` manifest, missing Wasm host when `kind=wasm`) fail closed at boot (`ServeError::Config`). Datapath errors map through existing `ServeError::status` (never 2xx). Key daemon down: TLS accept fails closed (no silent plaintext). Worker death: sup restarts that index only.

---

## 14. Testing obligations

- Unit: `Config::validate` rejects `engine=epoll` with `http2`/`http3`; default engine is epoll; native plugin refused (have).
- Integration: `epoll_smoke` remains; proto tests gated `#[cfg(feature = "proto")]`.
- Invariant: cache second hit does not increment a module counter (have, keep).
- Smuggling: `Content-Length` vs `Transfer-Encoding` rejected.
- Wasm: fuel exhaustion returns 503; cache hit still skips Wasm.
- Sup: SIGHUP two pids generations; drain respects timeout.
- Never bind 8082 in tests.

---

## 15. Out of scope (other sites, other crates)

- PaperRetrieval product logic; live 8082.
- Reverse proxy, upstream pool, gRPC bridge.
- Disk cache + cache-manager process (reopen as a new spec if a consumer needs it).
- AF_XDP site (real NIC); keep `EngineKind::Xdp` as a typed hole.
- FIPS extra TLS stacks.
- Level-III machine-checked CC-00 (C3). C2 review + tests only.

---

## 16. Self-review

- Placeholders: none. Forfeits are named. H-04 gates are named.
- Consistency: default engine epoll vs proto binary tokio; cache hit never Wasm; `.so` refused.
- Scope: one site (origin kernel). Proxy is another crate.
- Ambiguity: “win all” = 100% of S, not 100% of Web. “Smaller monolith” = TCB split, not Lua.
