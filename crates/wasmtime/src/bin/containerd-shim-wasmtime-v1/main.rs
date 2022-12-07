use containerd_shim as shim;
use containerd_shim_wasm::sandbox::ShimCli;
use runwasi::instance::Wasi as WasiInstance;

fn main() {
    shim::run::<ShimCli<WasiInstance, wasmtime::Engine>>("io.containerd.wasmtime.v1", None);
}
