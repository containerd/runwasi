use containerd_shim as shim;
use containerd_shim_wasm::sandbox::manager::Shim;

fn main() {
    shim::run::<Shim>("containerd-shim-wasmtimed-v1", None)
}
