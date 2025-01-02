//! Abstracts the sandboxing environment and execution context for a container.

pub mod cli;
pub mod error;
pub mod instance;
pub mod instance_utils;
pub mod shim;
pub mod sync;

pub use error::{Error, Result};
pub use instance::{Instance, InstanceConfig};
pub use shim::Cli as ShimCli;

pub(crate) mod containerd;
pub(crate) mod oci;
pub use oci::WasmLayer;

pub(crate) mod async_utils;
