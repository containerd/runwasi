#[cfg(not(target_os = "windows"))]
use containerd_shim_wamr::WamrInstance;
use containerd_shim_wasm::sandbox::cli::{revision, shim_main, version};

#[cfg(target_os = "windows")]
fn main() {
    panic!("WAMR shim is not supported on Windows");
}

#[cfg(not(target_os = "windows"))]
fn main() {
    shim_main::<WamrInstance>("wamr", version!(), revision!(), "v1", None);
}
