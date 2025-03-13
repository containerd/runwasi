#![doc = include_str!("../README.md")]

pub mod sandbox;
mod vendor;

#[cfg_attr(unix, path = "sys/unix/mod.rs")]
#[cfg_attr(windows, path = "sys/windows/mod.rs")]
pub(crate) mod sys;

pub use containerd_shim::Config;
pub use sandbox::async_utils::AmbientRuntime;
#[cfg(unix)]
pub use zygote;
