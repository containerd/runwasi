pub mod instance;

pub use instance::WasmEdgeInstance;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmedge_tests;
