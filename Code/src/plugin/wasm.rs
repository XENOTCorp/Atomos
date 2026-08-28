//! Wasmtime host for `wit/atomos-module.wit`. Fuel + epoch. Not on cache-hit.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

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

/// Fuel/epoch traps become [`ServeError::Capacity`]. Other wasm errors stay Module.
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
    if s.contains("fuel") || s.contains("epoch") {
        ServeError::Capacity
    } else {
        ServeError::Module(s.into_boxed_str())
    }
}

struct WasmMod {
    engine: Engine,
    component: Component,
    linker: Linker<()>,
    fuel: u64,
}

/// Compile `path` as a component implementing world `module`.
pub fn load(path: &Path, fuel: u64) -> Result<Arc<dyn Module>, ServeError> {
    let engine = shared_engine()?;
    start_epoch(engine);
    let bytes = std::fs::read(path)
        .map_err(|e| ServeError::Config(format!("wasm {}: {e}", path.display()).into()))?;
    let component = Component::from_binary(engine, &bytes)
        .map_err(|e| ServeError::Config(format!("wasm {}: {e}", path.display()).into()))?;
    let linker = Linker::new(engine);
    Ok(Arc::new(WasmMod {
        engine: engine.clone(),
        component,
        linker,
        fuel,
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
        let mut store = Store::new(&self.engine, ());
        store.epoch_deadline_trap();
        store.set_epoch_deadline(1_000);
        store.set_fuel(self.fuel).map_err(fuel_to_capacity)?;
        let _live = LiveGuard::enter();
        let guest = bindings::Module::instantiate(&mut store, &self.component, &self.linker)
            .map_err(fuel_to_capacity)?;
        let wit_req = request_from_in(req);
        match guest
            .atomos_module_handler()
            .call_handle(&mut store, &wit_req)
        {
            Ok(Ok(resp)) => Ok(out_from_response(resp)),
            Ok(Err(msg)) => Err(ServeError::Module(msg.into_boxed_str())),
            Err(e) => Err(fuel_to_capacity(e)),
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

    /// BLOCKED fixture: no wasm-tools/cargo-component in this environment, and
    /// wasmtime is built without `wat`, so a looping WIT component cannot be compiled here.
    #[test]
    #[ignore = "BLOCKED fixture: WIT component compile too heavy (no wasm-tools)"]
    fn wasm_fuel_exhaustion_is_capacity() {
        panic!("needs a looping atomos:module component");
    }
}
