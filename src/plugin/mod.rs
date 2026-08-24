//! Plug-in plane. Native modules register by name. Wasm is the sandboxed
//! hot-swap slot (WIT in `wit/atomos-module.wit`). `.so` load is refused.

mod manifest;
mod registry;
#[cfg(feature = "wasm")]
pub mod wasm;

pub use manifest::{PluginKind, PluginManifest};
pub use registry::{load_dir, reload};
