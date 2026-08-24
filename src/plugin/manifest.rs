//! Plugin directory JSON. One file per module.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// Already in the process (`Handler` registered in Rust). Manifest only names it.
    Builtin,
    /// WebAssembly component implementing `wit/atomos-module.wit`.
    Wasm,
    /// Explicitly rejected. Native `.so` is not a sandbox.
    Native,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub kind: PluginKind,
    /// Path to `.wasm` (kind=wasm) relative to the manifest file.
    #[serde(default)]
    pub path: Option<String>,
    /// If true, this module may be used as `pre_module` / `post_module`.
    #[serde(default)]
    pub hook: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wasm_manifest() {
        let m: PluginManifest =
            serde_json::from_str(r#"{"name":"echo","kind":"wasm","path":"echo.wasm"}"#).unwrap();
        assert_eq!(m.kind, PluginKind::Wasm);
        assert_eq!(m.path.as_deref(), Some("echo.wasm"));
    }
}
