pub mod instance;

pub use instance::WamrInstance;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wamr_tests;
