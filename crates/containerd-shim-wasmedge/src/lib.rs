pub mod instance;

pub use instance::WasmEdgeShim;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmedge_tests;
