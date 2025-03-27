#![doc = include_str!("../README.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/containerd/runwasi/e251de3307bbdc8bf3229020ea2ae2711f31aafa/art/logo/runwasi_logo_icon.svg"
)]

pub mod sandbox;
pub mod shim;

#[cfg_attr(unix, path = "sys/unix/mod.rs")]
#[cfg_attr(windows, path = "sys/windows/mod.rs")]
pub(crate) mod sys;

#[cfg(any(test, feature = "testing"))]
/// Utilities for writing shims tests.
/// You can use this to test your runwasi based shim.
pub mod testing;

#[cfg(test)]
/// Tests for runwasi's containerd-shim-wasm.
mod test;

pub(crate) mod containerd;
