#[cfg(unix)]
pub mod instance;

#[cfg(unix)]
pub use instance::WamrInstance;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wamr_tests;
