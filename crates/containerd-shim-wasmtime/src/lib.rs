mod http_proxy;
pub mod instance;

pub use instance::WasmtimeShim;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmtime_tests;
