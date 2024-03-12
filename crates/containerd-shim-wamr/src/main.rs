use containerd_shim_wamr::WamrInstance;
use containerd_shim_wasm::sandbox::cli::{revision, shim_main, version};

fn main() {
    shim_main::<WamrInstance>("wamr", version!(), revision!(), "v1", None);
}
