use containerd_shim_wasm::{revision, shim_main, version};
use containerd_shim_wasmer::WasmerEngine;

fn main() {
    shim_main::<WasmerEngine>("wasmer", version!(), revision!(), "v1", None);
}
