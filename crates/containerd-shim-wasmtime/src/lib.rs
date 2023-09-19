#[cfg_attr(unix, path = "instance_linux.rs")]
#[cfg_attr(windows, path = "instance_windows.rs")]
pub mod instance;

pub use instance::WasmtimeInstance;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmtime_tests;
