use wasmparser::Parser;

/// The type of a wasm binary.
pub enum WasmBinaryType {
    /// A wasm module.
    Module,
    /// A wasm component.
    Component,
}

impl WasmBinaryType {
    /// Returns the type of the wasm binary.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if Parser::is_component(bytes) {
            Some(Self::Component)
        } else if Parser::is_core_wasm(bytes) {
            Some(Self::Module)
        } else {
            None
        }
    }
}
