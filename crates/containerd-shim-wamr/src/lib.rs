#[cfg(unix)]
pub mod instance;

#[cfg(unix)]
pub use instance::WamrShim;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wamr_tests;
