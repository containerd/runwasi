use containerd_shim_wasm::{revision, shim_main, version};
use containerd_shim_wasmer::WasmerShim;

fn main() {
    shim_main::<WasmerShim>(version!(), revision!(), None);
}
