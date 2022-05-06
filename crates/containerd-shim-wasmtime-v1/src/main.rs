use containerd_shim as shim;
use containerd_shim_wasmtime_v1::sandbox::{ShimCli, WasiInstance};

fn main() {
    shim::run::<ShimCli<WasiInstance, wasmtime::Engine>>("io.containerd.wasmtime.v1", None);
}
