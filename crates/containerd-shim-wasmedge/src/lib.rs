pub mod instance;

pub use instance::WasmEdgeEngine;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmedge_tests;
