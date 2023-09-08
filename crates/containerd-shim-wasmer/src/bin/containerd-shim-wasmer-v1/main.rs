use containerd_shim as shim;
use containerd_shim_wasm::sandbox::ShimCli;
use containerd_shim_wasmer::instance::Wasi as WasiInstance;

fn main() {
    shim::run::<ShimCli<WasiInstance>>("io.containerd.wasmer.v1", None);
}
