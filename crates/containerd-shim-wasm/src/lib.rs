#![doc = include_str!("../doc/doc.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/containerd/runwasi/e251de3307bbdc8bf3229020ea2ae2711f31aafa/art/logo/runwasi_logo_icon.svg"
)]

pub mod container;
pub mod sandbox;
pub mod services;

#[cfg_attr(unix, path = "sys/unix/mod.rs")]
#[cfg_attr(windows, path = "sys/windows/mod.rs")]
pub(crate) mod sys;

pub use containerd_shim_wasm_macros::*;
pub use {containerd_shim as shim, ttrpc};
