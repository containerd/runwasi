use containerd_shim_wasm::Cli;
use containerd_shim_wasmer::WasmerShim;

fn main() {
    WasmerShim::run(None);
}
