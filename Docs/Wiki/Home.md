# Wiki

Atomos is an HTTP kernel. A consumer registers modules. A ruleset selects one module per request.

| Page | Content |
|---|---|
| [Overview](Overview.md) | What the kernel does. Two engines. |
| [Architecture](Architecture.md) | Planes. Pre/post. Hot-swap. Charts. |
| [Compile](Compile.md) | `compile.sh`. Device files. Portable Linux build. |
| [Requests](Requests.md) | `In`, `Out`, body types, cache directives. |
| [Rules](Rules.md) | Exact and prefix. exclude. Overlap is an error. |
| [Modules](Modules.md) | `Module` trait. pre and post. |
| [Atoms](Atoms.md) | Pure vs effectful. molecule lists. |
| [Control](Control.md) | `atomos-ctl`, socket, commands. |
| [Http2-Http3](Http2-Http3.md) | `atomos-proto`. TLS. Measured notes. |
| [Plugins](Plugins.md) | Manifest kinds. Wasm WIT path. `.so` is refused. |
| [Configuration](Configuration.md) | `config.json` fields. host overlay. hard bounds. |
| [Performance](Performance.md) | Release profile. governor. Hardware layout. |
| [Examples](Examples.md) | login_server, first_app, static_site, echo_api, loadgen. |
| [Limits](Limits.md) | Pipelining, WebSocket, H1 TLS, streaming on H1. |
| [Binaries](Binaries.md) | `atomos`, `atomos-proto`, `atomos-ctl`, `atomos-sup`, `atomos-keyd`. |

Getting started: [../Getting-Started.md](../Getting-Started.md).

Benchmarks: [../Benchmarks.md](../Benchmarks.md).

Maintain: [../Maintain.md](../Maintain.md).
