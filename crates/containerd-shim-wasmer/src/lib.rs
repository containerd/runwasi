pub mod instance;

pub use instance::WasmerShim;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmer_tests;
