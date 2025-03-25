#[cfg(not(target_os = "windows"))]
use containerd_shim_wamr::WamrEngine;
use containerd_shim_wasm::{revision, shim_main, version};

#[cfg(target_os = "windows")]
fn main() {
    panic!("WAMR shim is not supported on Windows");
}

#[cfg(not(target_os = "windows"))]
fn main() {
    shim_main::<WamrEngine>(version!(), revision!(), None);
}
