#[cfg(unix)]
pub mod instance;

pub use instance::WamrEngine;

#[cfg(unix)]
#[cfg(test)]
#[path = "tests.rs"]
mod wamr_tests;
