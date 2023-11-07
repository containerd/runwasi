use containerd_shim_wasm::sandbox::cli::{revision, shim_main, version};
use containerd_shim_wasmer::WasmerInstance;

fn main() {
    shim_main::<WasmerInstance>("wasmer", version!(), revision!(), "v1", None);
}
