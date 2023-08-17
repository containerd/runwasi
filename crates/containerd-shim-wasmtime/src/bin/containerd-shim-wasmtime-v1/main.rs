use containerd_shim as shim;
use containerd_shim_wasm::sandbox::ShimCli;
use containerd_shim_wasmtime::instance::Wasi as WasiInstance;

fn main() {
    shim::run::<ShimCli<WasiInstance>>("io.containerd.wasmtime.v1", None);
}
