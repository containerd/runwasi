use containerd_shim_wasm::Cli;
use containerd_shim_wasmedge::WasmEdgeShim;

fn main() {
    WasmEdgeShim::run(None);
}
