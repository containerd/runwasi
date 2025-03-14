pub mod instance;

pub use instance::WasmerEngine;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmer_tests;
