mod http_proxy;
pub mod instance;

pub use instance::WasmtimeInstance;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmtime_tests;
