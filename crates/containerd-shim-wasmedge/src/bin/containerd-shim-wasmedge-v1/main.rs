use containerd_shim as shim;
use containerd_shim_wasm::sandbox::ShimCli;
use containerd_shim_wasmedge::{parse_version, WasmEdgeInstance};

fn main() {
    parse_version();
    shim::run::<ShimCli<WasmEdgeInstance>>("io.containerd.wasmedge.v1", None);
}
