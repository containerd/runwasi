//! This module provides abstractions for managing WebAssembly containers in a sandboxed environment.
//!
//! This module gives you complete control over the container lifecycle and sandboxing,
//! compared to the higher-level container module. It's useful when you need:
//!
//! - Custom sandboxing requirements
//! - Direct control over container lifecycle
//! - Support MacOS or Windows
//!
//! There are also some downsides to using this module:
//!
//! - No precompilation out-of-the-box
//! - Does not support for native Linux containers out-of-the-box
//! - Requires manual handling of cgroup setup
//!
//! ## Key Components
//!
//! - [`Instance`]: Core trait for implementing container lifecycle management
//! - [`ShimCli`]: Command-line interface implementation for containerd shims
//! - [`WasmLayer`]: Represents WebAssembly layers in OCI containers
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use containerd_shim_wasm::sandbox::{Instance, InstanceConfig, Error};
//! use containerd_shim_wasm::container::{Engine, RuntimeContext};
//! use chrono::{DateTime, Utc};
//! use std::time::Duration;
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
//!         Ok(0)
//!     }
//! }
//!
//! struct MyInstance {
//!     engine: MyEngine,
//! }
//!
//! impl Instance for MyInstance {
//!     async fn new(id: String, cfg: &InstanceConfig) -> Result<Self, Error> {
//!         Ok(MyInstance { engine: MyEngine })
//!     }
//!
//!     async fn start(&self) -> Result<u32, Error> {
//!         Ok(1)
//!     }
//!
//!     async fn kill(&self, signal: u32) -> Result<(), Error> {
//!         Ok(())
//!     }
//!
//!     async fn delete(&self) -> Result<(), Error> {
//!         Ok(())
//!     }
//!
//!     async fn wait(&self) -> (u32, DateTime<Utc>) {
//!         (0, Utc::now())
//!     }
//! }
//! ```
//!
//! For simpler use cases, consider using the [`crate::container`] module instead.

pub mod cli;
pub mod error;
pub mod instance;
pub mod instance_utils;
pub mod shim;
pub mod sync;

pub use error::{Error, Result};
pub use instance::{Instance, InstanceConfig};
pub use shim::{Cli as ShimCli, Config};

pub(crate) mod containerd;
pub(crate) mod oci;
pub use oci::WasmLayer;

pub(crate) mod async_utils;
