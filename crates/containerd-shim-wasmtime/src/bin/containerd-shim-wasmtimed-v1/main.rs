use containerd_shim as shim;
use containerd_shim_wasm::sandbox::manager::Shim;
use containerd_shim_wasmtime::parse_version;

fn main() {
    parse_version();
    shim::run::<Shim>("containerd-shim-wasmtimed-v1", None)
}
