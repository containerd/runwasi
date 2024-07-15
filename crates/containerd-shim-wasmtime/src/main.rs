use containerd_shim_wasm::sandbox::cli::{revision, shim_main_with_otel, version};
use containerd_shim_wasmtime::WasmtimeInstance;

fn main() {
    shim_main_with_otel::<WasmtimeInstance>("wasmtime", version!(), revision!(), "v1", None);
}
