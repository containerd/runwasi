//! Abstracts the sandboxing environment and execution context for a container.

use crate::services::sandbox;

pub mod error;
pub mod instance;
pub mod instance_utils;
pub mod manager;
pub mod shim;
pub mod stdio;

pub use error::{Error, Result};
pub use instance::{Instance, InstanceConfig};
pub use manager::{Sandbox as SandboxService, Service as ManagerService};
pub use shim::{Cli as ShimCli, Local};
pub use stdio::Stdio;

pub(crate) mod oci;

pub mod testutil;
