use containerd_shim as shim;
use containerd_shim_wasm::sandbox::manager::Shim;
use containerd_shim_wasmedge::parse_version;

fn main() {
    parse_version();
    shim::run::<Shim>("containerd-shim-wasmedged-v1", None)
}
