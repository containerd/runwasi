//! This module contains an API for building WebAssembly shims running on top of containers.
//! Unlike the `sandbox` module, this module delegates many of the actions to the container runtime.
//!
//! This has some advantages:
//! * Simplifies writing new shims, get you up and running quickly
//! * The complexity of the OCI spec is already taken care of
//!
//! But it also has some disadvantages:
//! * Runtime overhead in in setting up a container
//! * Less customizable
//! * Currently only works on Linux

mod context;
mod engine;
mod path;
mod wasm;

pub(crate) use context::WasiContext;
pub use context::{Entrypoint, RuntimeContext, Source};
pub use engine::{Engine, PrecompiledLayer};
pub use instance::Instance;
pub use path::PathResolve;
pub use wasm::WasmBinaryType;

use crate::sys::container::instance;

#[cfg(test)]
mod tests;
