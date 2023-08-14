//! Abstracts the sandboxing environment and execution context for a container.

use crate::services::sandbox;

// pub mod cgroups;
pub mod error;
// pub mod exec;
pub mod instance;
pub mod instance_utils;
#[cfg(feature = "libcontainer")]
pub mod libcontainer_instance;
pub mod manager;
pub mod shim;
#[cfg(feature = "libcontainer")]
pub use libcontainer_instance::LibcontainerInstance;

pub use error::{Error, Result};
pub use instance::{EngineGetter, Instance, InstanceConfig};
pub use manager::{Sandbox as SandboxService, Service as ManagerService};
pub use shim::{Cli as ShimCli, Local};

pub mod oci;

pub mod testutil;
