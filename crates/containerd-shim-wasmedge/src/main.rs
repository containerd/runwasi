use containerd_shim_wasm::{revision, shim_main, version};
use containerd_shim_wasmedge::WasmEdgeEngine;

fn main() {
    shim_main::<WasmEdgeEngine>("wasmedge", version!(), revision!(), "v1", None);
}
