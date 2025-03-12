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
//!
//! ## Key Components
//!
//! - [`Engine`]: The core trait for implementing Wasm runtimes
//! - [`RuntimeContext`]: The context for running WASI modules
//!
//! ## Example Usage
//!
//! ```rust
//! use containerd_shim_wasm::container::{Engine, RuntimeContext};
//! use anyhow::Result;
//!
//! #[derive(Clone, Default)]
//! struct MyEngine;
//!
//! impl Engine for MyEngine {
//!     fn name() -> &'static str {
//!         "my-engine"
//!     }
//!
//!     fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
//!         let args = ctx.args();
//!         let envs = ctx.envs();
//!         let entrypoint = ctx.entrypoint();
//!         let platform = ctx.platform();
//!
//!         Ok(0)
//!     }
//! }
//! ```

mod context;
mod engine;
pub mod log;
mod path;
mod wasm;

pub(crate) use context::WasiContext;
pub use context::{Entrypoint, RuntimeContext, Source};
pub use engine::Engine;
pub use instance::Instance;
pub(crate) use path::PathResolve;
pub use wasm::WasmBinaryType;

use crate::sys::container::instance;

#[cfg(test)]
mod tests;
