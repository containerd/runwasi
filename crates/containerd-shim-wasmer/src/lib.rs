#[cfg_attr(unix, path = "instance_linux.rs")]
#[cfg_attr(windows, path = "instance_windows.rs")]
pub mod instance;

pub use instance::WasmerInstance;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmer_tests;
