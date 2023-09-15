use containerd_shim_wasm::sandbox::cli::{revision, shim_main, version};
use containerd_shim_wasmedge::WasmEdgeInstance;

fn main() {
    shim_main::<WasmEdgeInstance>("WasmEdge", version!(), revision!(), None);
}
