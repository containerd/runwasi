use containerd_shim_wasm::{revision, shim_main, version};
use containerd_shim_wasmedge::WasmEdgeShim;

fn main() {
    shim_main::<WasmEdgeShim>(version!(), revision!(), None);
}
