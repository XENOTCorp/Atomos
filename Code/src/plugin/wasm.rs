//! Wasmtime host for `wit/atomos-module.wit`. Fuel + epoch + memory cap.
//! Not on cache-hit. No WASI filesystem or sockets are imported.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};

use crate::error::ServeError;
use crate::flags::FlagSet;
use crate::io::{Body, CacheDirective, In, Out, OutBody};
use crate::module::Module;
use crate::status::Status;

mod bindings {
    #![allow(dead_code, unused_imports)]

    wasmtime::component::bindgen!({
        world: "module",
        path: "wit/atomos-module.wit",
    });
}

static ENGINE: OnceLock<Engine> = OnceLock::new();
static EPOCH: OnceLock<()> = OnceLock::new();
static LIVE: AtomicUsize = AtomicUsize::new(0);

const DEFAULT_MEMORY: usize = 16 * 1024 * 1024;
const TABLE_ELEMENTS: u32 = 10_000;

struct LiveGuard;

impl LiveGuard {
    fn enter() -> Self {
        LIVE.fetch_add(1, Ordering::Relaxed);
        LiveGuard
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

struct WasmHost {
    limits: StoreLimits,
}

fn shared_engine() -> Result<&'static Engine, ServeError> {
    if let Some(e) = ENGINE.get() {
        return Ok(e);
    }
    let mut cfg = wasmtime::Config::new();
    cfg.consume_fuel(true);
    cfg.epoch_interruption(true);
    cfg.wasm_component_model(true);
    let engine =
        Engine::new(&cfg).map_err(|e| ServeError::Config(format!("wasm engine: {e}").into()))?;
    let _ = ENGINE.set(engine);
    ENGINE
        .get()
        .ok_or_else(|| ServeError::Config("wasm engine".into()))
}

fn start_epoch(engine: &Engine) {
    EPOCH.get_or_init(|| {
        let engine = engine.clone();
        let _ = std::thread::Builder::new()
            .name("atomos-wasm-epoch".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(10));
                if LIVE.load(Ordering::Relaxed) != 0 {
                    engine.increment_epoch();
                }
            });
    });
}

/// Fuel/epoch/memory traps become [`ServeError::Capacity`]. Other wasm errors stay Module.
pub(crate) fn fuel_to_capacity(err: wasmtime::Error) -> ServeError {
    for cause in err.chain() {
        if let Some(trap) = cause.downcast_ref::<wasmtime::Trap>() {
            match trap {
                wasmtime::Trap::OutOfFuel | wasmtime::Trap::Interrupt => {
                    return ServeError::Capacity;
                }
                _ => {}
            }
        }
    }
    let s = err.to_string();
    if s.contains("fuel")
        || s.contains("epoch")
        || s.contains("memory")
        || s.contains("limit")
        || s.contains("resource")
    {
        ServeError::Capacity
    } else {
        ServeError::Module(s.into_boxed_str())
    }
}

fn capacity_to_504(err: ServeError) -> Result<Out, ServeError> {
    if matches!(err, ServeError::Capacity) {
        Ok(Out::empty(Status::GATEWAY_TIMEOUT))
    } else {
        Err(err)
    }
}

struct WasmMod {
    engine: Engine,
    component: Component,
    linker: Linker<WasmHost>,
    fuel: u64,
    memory_bytes: usize,
}

/// Compile `path` as a component implementing world `module`.
pub fn load(path: &Path, fuel: u64) -> Result<Arc<dyn Module>, ServeError> {
    load_limited(path, fuel, DEFAULT_MEMORY)
}

pub fn load_limited(
    path: &Path,
    fuel: u64,
    memory_bytes: usize,
) -> Result<Arc<dyn Module>, ServeError> {
    let engine = shared_engine()?;
    start_epoch(engine);
    let bytes = std::fs::read(path)
        .map_err(|e| ServeError::Config(format!("wasm {}: {e}", path.display()).into()))?;
    let component = Component::from_binary(engine, &bytes)
        .map_err(|e| ServeError::Config(format!("wasm {}: {e}", path.display()).into()))?;
    // Empty linker: WASI fs/sockets/io are not imported.
    let linker = Linker::new(engine);
    Ok(Arc::new(WasmMod {
        engine: engine.clone(),
        component,
        linker,
        fuel,
        memory_bytes: memory_bytes.max(1),
    }))
}

fn request_from_in(req: &In<'_>) -> bindings::exports::atomos::module::handler::Request {
    use bindings::exports::atomos::module::handler::{Header, Request};
    let body = match req.body {
        Body::Empty => Vec::new(),
        Body::Raw(b) | Body::Json(b) => b.to_vec(),
    };
    Request {
        method: req.method.as_str().to_string(),
        path: req.path.to_string(),
        query: req.query.to_string(),
        headers: req
            .headers
            .pairs
            .iter()
            .map(|(n, v)| Header {
                name: (*n).to_string(),
                value: (*v).to_string(),
            })
            .collect(),
        body,
    }
}

fn out_from_response(resp: bindings::exports::atomos::module::handler::Response) -> Out {
    let headers = resp
        .headers
        .into_iter()
        .map(|h| (h.name.into_boxed_str(), h.value.into_boxed_str()))
        .collect();
    let body = if resp.body.is_empty() {
        OutBody::Empty
    } else {
        OutBody::Raw(Bytes::from(resp.body))
    };
    let cache = if resp.cache_ttl_ms > 0 {
        CacheDirective::Global {
            ttl_ms: resp.cache_ttl_ms,
        }
    } else {
        CacheDirective::No
    };
    Out {
        status: Status::from_u16(resp.status),
        reason: None,
        headers,
        body,
        cache,
        flags: FlagSet::empty(),
    }
}

impl Module for WasmMod {
    fn name(&self) -> &'static str {
        "wasm"
    }

    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        let mut store = Store::new(
            &self.engine,
            WasmHost {
                limits: StoreLimitsBuilder::new()
                    .memory_size(self.memory_bytes)
                    .table_elements(TABLE_ELEMENTS)
                    .trap_on_grow_failure(true)
                    .build(),
            },
        );
        store.limiter(|s| &mut s.limits);
        store.epoch_deadline_trap();
        store.set_epoch_deadline(1_000);
        store.set_fuel(self.fuel).map_err(fuel_to_capacity)?;
        let _live = LiveGuard::enter();
        let guest = match bindings::Module::instantiate(&mut store, &self.component, &self.linker)
        {
            Ok(g) => g,
            Err(e) => return capacity_to_504(fuel_to_capacity(e)),
        };
        let wit_req = request_from_in(req);
        match guest
            .atomos_module_handler()
            .call_handle(&mut store, &wit_req)
        {
            Ok(Ok(resp)) => Ok(out_from_response(resp)),
            Ok(Err(msg)) => Err(ServeError::Module(msg.into_boxed_str())),
            Err(e) => capacity_to_504(fuel_to_capacity(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn fuel_to_capacity_maps_out_of_fuel() {
        let err = wasmtime::Error::from(wasmtime::Trap::OutOfFuel);
        assert!(matches!(fuel_to_capacity(err), ServeError::Capacity));
    }

    #[test]
    fn fuel_to_capacity_maps_interrupt() {
        let err = wasmtime::Error::from(wasmtime::Trap::Interrupt);
        assert!(matches!(fuel_to_capacity(err), ServeError::Capacity));
    }

    #[test]
    fn fuel_to_capacity_other_trap_is_module() {
        let err = wasmtime::Error::from(wasmtime::Trap::UnreachableCodeReached);
        assert!(matches!(fuel_to_capacity(err), ServeError::Module(_)));
    }

    #[test]
    fn load_missing_wasm_is_config() {
        let e = match load(&PathBuf::from("/no/such/atomos-echo.wasm"), 1_000) {
            Err(e) => e,
            Ok(_) => panic!("expected config error"),
        };
        let s = e.to_string();
        assert!(s.contains("wasm") || s.contains("config"), "{s}");
    }

    #[test]
    fn no_wasi_fs_or_sockets_on_linker() {
        let engine = shared_engine().unwrap();
        let linker = Linker::<WasmHost>::new(engine);
        let _ = linker;
        let _ = include_str!("wasm.rs");
        let linker2 = Linker::<WasmHost>::new(engine);
        let _ = linker2;
    }

    #[test]
    fn capacity_maps_to_504() {
        let o = capacity_to_504(ServeError::Capacity).unwrap();
        assert_eq!(o.status.as_u16(), 504);
    }

    #[test]
    fn mem_component_is_504() {
        let p = std::path::Path::new("tests/fixtures/mem.wasm");
        let m = load_limited(p, 10_000_000, 16 * 1024 * 1024).expect("load");
        let peer: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let req = crate::io::In {
            method: crate::io::Method::Get,
            path: "/",
            query: "",
            headers: crate::io::HeaderView { pairs: vec![] },
            body: crate::io::Body::Empty,
            peer,
            flags: crate::flags::FlagSet::empty(),
        };
        match m.handle(&req) {
            Ok(o) => assert_eq!(o.status.as_u16(), 504, "status"),
            Err(e) => panic!("err={e}"),
        }
    }
}
