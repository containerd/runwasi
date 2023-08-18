use containerd_shim as shim;
use containerd_shim_wasm::sandbox::ShimCli;
use containerd_shim_wasmtime::{parse_version, WasmtimeInstance};

fn main() {
    parse_version();
    shim::run::<ShimCli<WasmtimeInstance>>("io.containerd.wasmtime.v1", None);
}
