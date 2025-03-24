use containerd_shim_wasm::{revision, shim_main, version};
use containerd_shim_wasmtime::WasmtimeShim;

fn main() {
    shim_main::<WasmtimeShim>(version!(), revision!(), None);
}
