use containerd_shim_wasm::sandbox::cli::{revision, shim_main, version};
use containerd_shim_wasmtime::WasmtimeInstance;

fn main() {
    shim_main::<WasmtimeInstance>("wasmtime", version!(), revision!(), "v1", None);
}
