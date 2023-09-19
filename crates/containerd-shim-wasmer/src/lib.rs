pub mod instance;

pub use instance::WasmerInstance;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wasmer_tests;
