use containerd_shim_wasm::sandbox::cli::{revision, shim_main, version};
use containerd_shim_wasmtime::WasmtimeInstance;

fn main() {
    containerd_shim_wasmtime::instance::init();
    shim_main::<WasmtimeInstance>("wasmtime", version!(), revision!(), "v1", None);
}
