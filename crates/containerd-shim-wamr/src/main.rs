use containerd_shim_wamr::WamrInstance;
use containerd_shim_wasm::sandbox::cli::{revision, shim_main, version};

#[cfg(target_os = "windows")]
fn main() {
    compile_error!("This shim binary only supports Unix");
}

#[cfg(not(target_os = "windows"))]
fn main() {
    shim_main::<WamrInstance>("wamr", version!(), revision!(), "v1", None);
}
