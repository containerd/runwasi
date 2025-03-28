use containerd_shim_wasm::shim::Cli;
use containerd_shim_wasmer::WasmerShim;

fn main() {
    WasmerShim::run(None);
}
