/// The type of a wasm binary.
pub enum WasmBinaryType {
    Module,
    Component,
}

/// from: https://github.com/bytecodealliance/wasm-tools/blob/main/crates/wasmparser/src/parser.rs
pub(crate) const WASM_MAGIC_NUMBER: &[u8; 4] = b"\0asm";

pub(crate) const KIND_MODULE: u16 = 0x00;
pub(crate) const KIND_COMPONENT: u16 = 0x01;

pub(crate) const WASM_MODULE_VERSION: u16 = 0x1;
pub(crate) const WASM_COMPONENT_VERSION: u16 = 0xd;

const COMPONENT_HEADER: [u8; 8] = [
    WASM_MAGIC_NUMBER[0],
    WASM_MAGIC_NUMBER[1],
    WASM_MAGIC_NUMBER[2],
    WASM_MAGIC_NUMBER[3],
    WASM_COMPONENT_VERSION.to_le_bytes()[0],
    WASM_COMPONENT_VERSION.to_le_bytes()[1],
    KIND_COMPONENT.to_le_bytes()[0],
    KIND_COMPONENT.to_le_bytes()[1],
];

const MODULE_HEADER: [u8; 8] = [
    WASM_MAGIC_NUMBER[0],
    WASM_MAGIC_NUMBER[1],
    WASM_MAGIC_NUMBER[2],
    WASM_MAGIC_NUMBER[3],
    WASM_MODULE_VERSION.to_le_bytes()[0],
    WASM_MODULE_VERSION.to_le_bytes()[1],
    KIND_MODULE.to_le_bytes()[0],
    KIND_MODULE.to_le_bytes()[1],
];

impl WasmBinaryType {
    /// Returns the type of the wasm binary.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&COMPONENT_HEADER) {
            Some(Self::Component)
        } else if bytes.starts_with(&MODULE_HEADER) {
            Some(Self::Module)
        } else {
            None
        }
    }
}
