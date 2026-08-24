# What Atomos still lacks

Inventory against h2o, nginx, Pingora, picohttpparser, neverbleed, Wasm
components, Seastar/F-Stack/AF_XDP (USENIX ATC 2025 stacks). “Minimize the
difference” means each row gets a plane, not a one-loop miracle.

Status: **gap** (not built), **slot** (typed, not linked), **have**.

## Process and isolation (nginx)

| Item | Status | Notes |
|---|---|---|
| Master process, workers as **processes** | slot | `atomos-sup` spawns children; not privileged bind + fork inherited fds |
| Graceful reload (new workers, drain old) | partial | SIGTERM drain 2s then KILL; not two-generation SIGHUP |
| Shared memory across workers (cache, TLS sessions) | gap | our cache is **thread-local**; nginx `shared:SSL` |
| Worker crash ≠ process death of all cores | slot | sup restarts one child; in-process tokio still `panic=abort` **all threads** |
| `worker_shutdown_timeout` / drain keep-alives | gap | |
| Cache manager / loader processes | gap | nginx extras; skip until disk cache exists |
| `accept_mutex` / `EPOLLEXCLUSIVE` | gap | we use SO_REUSEPORT already (**have**) |

## I/O engine (h2o / nginx worker)

| Item | Status | Notes |
|---|---|---|
| One event loop per core, **no spawn-per-conn** | partial | `engine=epoll` has it; tokio path still spawns |
| Run-to-completion `write` on cache hit | have | epoll: `write` on the worker; tokio still a Future |
| picohttpparser-class H1 (stateless, no alloc, SSE4.2) | gap | httparse; same *idea* (borrow buf) but not pico’s SIMD |
| Connection = slot in an array, not a Future | have | epoll `HashMap<fd, Conn>` |
| `writev` / kTLS / zerocopy | gap | h2o socket zerocopy + optional kTLS |
| `recvmmsg`/`sendmmsg` | gap | |
| Engine plug `epoll` | have | `EngineKind::Epoll` HTTP/1.1 |
| Engine plug AF_XDP / DPDK | slot | `EngineKind::Xdp`; loopback cannot use this |
| Userspace TCP (F-Stack, Seastar native) | gap | ATC 2025: no stack wins all sizes; extra crate only with a NIC |

## HTTP/2 and HTTP/3 (h2o)

| Item | Status | Notes |
|---|---|---|
| H2 state machine on the worker thread | gap | `h2` crate + spawned streams |
| Prioritized streams / proper flow control tuning | gap | |
| H3 at h2o/quicly class | gap | quinn; ~5k rps on 2-core here |
| ALPN-selected protocol handler registry (h2o style) | partial | peek 0x16 / PRI in `serve` |
| HTTP/2 as the *fast* path | gap | H1 pipeline still wins on this box |

## TLS and keys (h2o neverbleed / Pingora)

| Item | Status | Notes |
|---|---|---|
| rustls | have | |
| Private keys in a **separate process** (neverbleed) | gap | Heartbleed-class isolation |
| OCSP stapling | gap | |
| TLS session cache **shared** across workers | gap | |
| FIPS / BoringSSL / s2n option (Pingora) | gap | rustls-only; Pingora offers openssl/boring/s2n/rustls |
| Ticket key rotation | gap | |
| Lazy TLS until ClientHello | have | h2c does not build rustls |

## Security / jail

| Item | Status | Notes |
|---|---|---|
| Loopback default, non-loopback opt-in | have | |
| Control Unix socket 0600 | have | |
| Socket **not** in `/tmp` | have | `$XDG_RUNTIME_DIR` / `/run/user/<uid>` |
| Landlock / seccomp allowlist | gap | |
| `chroot` / user namespace after bind | gap | nginx master binds 443 as root then drops |
| SCM_CREDENTIALS / `SO_PEERCRED` on ctl | have | same EUID (or uid 0) |
| Capability-dropped worker | partial | `drop_user` / `drop_group` + `no_new_privs`; not capset |
| Fuzz harness (preface, JSON depth, H2) | gap | |
| Request smuggling tests | gap | |
| Privilege-separated config parse | gap | |

## Modularity and hot-swap

| Item | Status | Notes |
|---|---|---|
| Disjoint JSON rules, `arc-swap` reload | have | |
| `Router::insert` ArcSwap native modules | have | |
| Plugin dir + JSON manifests | have | |
| Wasm component WIT `handle(In)→Out` | slot | `wit/atomos-module.wit`; host not linked |
| Wasmtime fuel + epoch interrupt | gap | needed before Wasm on any path |
| **No** `.so` | have | `kind: native` refused |
| Cache hit never calls plugin | have | |
| `pre`/`post` as named modules + `bind_hooks` | have | |
| mruby/lua (h2o/nginx) | won’t | Wasm is the extension language |
| Dynamic `load_module` nginx-style C | won’t | |

## Proxy / scale (Pingora) — stay out of this crate

| Item | Status | Notes |
|---|---|---|
| Upstream connection pool | won’t | consumer |
| Retry / failover / ketama | won’t | |
| gRPC / websocket proxy | gap / won’t | |
| Work-stealing vs pinned (Pingora has both) | have pin | stealing would fight cache TLS |
| 40M rps planet edge | n/a | hardware + anycast |

## Correctness / bounds (keep)

| Item | Status |
|---|---|
| RSS governor | have |
| JSON depth / body cap | have |
| GET without POST mutex | have |
| Named cache epoch | have |
| Full cache keys (not u64) | have |
| `cpu_fraction` observed | have |
| `queue_cap` kernel | removed (consumer) |

## Ops

| Item | Status | Notes |
|---|---|---|
| CLI / JSON ctl, no TUI | have | |
| `atomos-sup` | partial | restart-on-death + SIGTERM drain |
| Metrics (Prometheus) | gap | Pingora has a crate |
| Access log / structured | gap | tracing only |
| Health of workers to master | gap | |
| Host facts from `/proc` | have | `scripts/atomos-host.sh` |

## How to close gaps without mixing planes

1. **Isolation:** finish `atomos-sup` drain/reload; workers as processes; TLS keys in a neverbleed-like helper.
2. **H1 speed:** implement `EngineKind::Epoll` (no tokio, no spawn). Target canned-C ±20% on this box.
3. **H2:** only after epoll; dedicated codec on the worker thread, not `spawn` per stream.
4. **Wasm:** wasmtime component, fuel, **never on cache hit**.
5. **XDP:** third engine, real NIC only (ATC 2025: bypass stacks are not uniformly faster).
6. **Jail:** Landlock+seccomp+runtime-dir socket before any public bind.

Do not put 2–5 in `ops/` or Wasm on the wire cache path.
