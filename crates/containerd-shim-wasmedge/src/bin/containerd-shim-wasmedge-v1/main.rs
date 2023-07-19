use containerd_shim as shim;
use containerd_shim_wasm::sandbox::ShimCli;
use containerd_shim_wasmedge::instance::Wasi as WasiInstance;

fn main() {
    shim::run::<ShimCli<WasiInstance, wasmedge_sdk::Vm>>("io.containerd.wasmedge.v1", None);
}
