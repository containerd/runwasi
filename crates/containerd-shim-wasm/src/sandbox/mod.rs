//! Abstracts the sandboxing environment and execution context for a container.

use crate::services::sandbox;

pub mod cli;
pub mod error;
pub mod instance;
pub mod instance_utils;
pub mod manager;
pub mod shim;
pub mod stdio;
pub mod sync;

pub use error::{Error, Result};
pub use instance::{Instance, InstanceConfig};
pub use manager::{Sandbox as SandboxService, Service as ManagerService};
pub use shim::Cli as ShimCli;
pub use stdio::Stdio;

pub(crate) mod containerd;
pub(crate) mod oci;
pub use oci::WasmLayer;
