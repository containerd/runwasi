use containerd_shim as shim;
use containerd_shim_wasmtime_v1::sandbox::manager::Shim;

fn main() {
    shim::run::<Shim>("containerd-shim-wasmtimed-v1", None)
}
