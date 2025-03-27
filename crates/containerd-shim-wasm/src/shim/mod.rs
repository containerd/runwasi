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
//! - [`Shim`]: The trait for implementing the shim entrypoint
//! - [`Sandbox`](crate::sandbox::Sandbox): The core trait for implementing Wasm runtimes
//! - [`RuntimeContext`](crate::sandbox::context::RuntimeContext): The context for running WASI modules
//!
//! ## Version Information
//!
//! The module provides two macros for version information:
//!
//! - [`version!()`](crate::shim::version) - Returns the crate version from Cargo.toml and
//!   Git revision hash, if available.
//!
//! ## Example Usage
//!
//! ```rust
//! use containerd_shim_wasm::shim::Shim;
//! use containerd_shim_wasm::sandbox::Sandbox;
//! use containerd_shim_wasm::sandbox::context::RuntimeContext;
//! use anyhow::Result;
//!
//! struct MyShim;
//!
//! #[derive(Default)]
//! struct MySandbox;
//!
//! impl Shim for MyShim {
//!     type Sandbox = MySandbox;
//!
//!     fn name() -> &'static str {
//!         "my-shim"
//!     }
//! }
//!
//! impl Sandbox for MySandbox {
//!     async fn run_wasi(&self, ctx: &impl RuntimeContext) -> Result<i32> {
//!         let args = ctx.args();
//!         let envs = ctx.envs();
//!         let entrypoint = ctx.entrypoint();
//!         let platform = ctx.platform();
//!
//!         Ok(0)
//!     }
//! }
//! ```

#[allow(clippy::module_inception)]
mod shim;

pub(crate) use instance::Instance;
pub use shim::{Compiler, Shim, Version};

use crate::sys::container::instance;

#[cfg(test)]
mod tests;

// This is used in containerd::Client tests
#[cfg(test)]
pub(crate) use shim::NO_COMPILER;

pub(crate) mod cli;

pub use cli::Cli;
pub use containerd_shimkit::{Config, shim_version as version};
