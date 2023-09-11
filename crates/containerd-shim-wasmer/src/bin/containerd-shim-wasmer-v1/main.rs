use containerd_shim as shim;
use containerd_shim_wasm::sandbox::ShimCli;
use containerd_shim_wasmer::{parse_version, WasmerInstance};

fn main() {
    parse_version();
    shim::run::<ShimCli<WasmerInstance>>("io.containerd.wasmer.v1", None);
}
