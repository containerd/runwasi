use containerd_shim_wasm::shim::Cli;
use containerd_shim_wasmtime::WasmtimeShim;

fn main() {
    WasmtimeShim::run(None);
}
