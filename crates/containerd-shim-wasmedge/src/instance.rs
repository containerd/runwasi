use containerd_shim_wasm::{cfg_unix, cfg_windows};

cfg_unix! {
    pub mod instance_linux;
    pub use instance_linux::Wasi;
}

cfg_windows! {
    pub mod instance_windows;
    pub use instance_windows::Wasi;
}
