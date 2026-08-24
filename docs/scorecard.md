# Axes: where Atomos wins vs where it does not

In-scope origin-kernel axes from [lack.md](lack.md). Excludes **won’t** (proxy, lua, `.so`) and **n/a** (planet anycast).

**59 axes.** Count after epoll-default H1, blocking `epoll::run`, `atomos-proto`
coproduct, CL+TE smuggling, sup drain timeout + SIGHUP generation.

| Bucket | Count | Share |
|---|---:|---:|
| **Winning (have)** | 28 | **47%** |
| Partial (same idea, not the competitor’s shape) | 2 | 3% |
| Slot (typed, not yet the win) | 3 | 5% |
| **Losing (gap)** | 26 | **44%** |

Winning + partial = **51%**. Losing + slot = **49%**.

Moved to have this round: default epoll H1 as the product (not a slot),
`I_engine` (epoll ⇏ h2/h3), blocking RTC join, smuggling CL+TE, configurable
drain timeout. Spawn-per-conn is no longer the H1 path. Wasm host, keyd, full
Landlock BPF still gap/slot.

You do **not** excel on **26** full gaps (plus 3 slots). That is the set to shrink. You already **win 28**.

## Winning (28)

Memory safety (Rust), loopback default, ctl Unix 0600, **socket under `$XDG_RUNTIME_DIR`**, SO_PEERCRED, `NO_NEW_PRIVS` + optional `drop_user`, no regex ReDoS, body/JSON caps, RSS governor, GET without POST mutex, named cache epoch, full cache keys, rustls, lazy TLS, rules `arc-swap`, `Router::insert` hot-swap, plugin manifests, `.so` refused, cache hit skips plugins, `bind_hooks`, CPU pin, SO_REUSEPORT, host facts from `/proc`, CLI-only ctl, observed `cpu_fraction`, **default epoll H1 product**, **I_engine** (epoll ⇏ h2/h3), blocking RTC join, CL+TE smuggling reject.

## Partial (2)

H1 cache hit is one write on the epoll worker (have). Tokio path is proto-only
and still a Future. ALPN/preface dispatch exists, not h2o’s protocol registry.

## Slot (3) — not winning yet

Wasm WIT (host unlinked), XDP engine, crash isolation while **inside** one tokio proto process (`panic=abort` still process-wide there; H1 workers are separate threads/processes under `atomos-sup`).

## Losing (26) — you do not excel

**Isolation / nginx (3):** SIGHUP two-generation is coded but not load-tested as the product; shared-memory TLS/cache; cache manager process. Drain timeout is now config (**have**).

**I/O / h2o (5):** picohttpparser SIMD; `writev`/kTLS/zerocopy; `recvmmsg`; userspace TCP; spawn-per-conn on **tokio proto** path (H1 epoll does not spawn).

**H2/H3 (4):** per-thread H2 codec; stream priority; H3 vs quicly; H2 as fast path.

**TLS (5):** neverbleed key process; OCSP; shared session cache; FIPS/BoringSSL/s2n; ticket rotation.

**Jail (4):** Landlock FS (path noted, not ABI-restrict); chroot/user ns; fuzz. cap-drop extra caps and smuggling CL+TE **moved toward have**. (socket-not-tmp, peer cred, no_new_privs already winning.)

**Wasm (1):** wasmtime fuel/epoch.

**Ops (3):** Prometheus; access log; worker heartbeat to master.

**H1 epoll** is now **have** for the speed engine (was slot). Tokio path still loses spawn-per-conn.

## Percentage story

| If you count… | You win |
|---|---|
| All 59 in-scope | **41%** have, **44%** have+partial |
| Safety / bounds / modularity only (~20) | **~80%+** |
| H2/H3/silicon/proxy only (~15) | **~0–10%** |

The plan shrinks the **56%** non-wins in this order: jail (done: runtime dir, drop, peer cred) → epoll H1 (done: linked) → Wasm host → SIGHUP two-generation reload → H2 codec → XDP. It will **not** turn 41% into 100%. The remaining ~half is H2/H3 class, neverbleed, Landlock, fuzz, and metrics — or axes we **forfeit** (Pingora proxy).
