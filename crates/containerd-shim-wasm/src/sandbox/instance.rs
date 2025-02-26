//! Abstractions for running/managing a wasm/wasi instance.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::Error;
use crate::sandbox::shim::Config;

/// Generic options builder for creating a wasm instance.
/// This is passed to the `Instance::new` method.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct InstanceConfig {
    /// Optional stdin named pipe path.
    pub stdin: PathBuf,
    /// Optional stdout named pipe path.
    pub stdout: PathBuf,
    /// Optional stderr named pipe path.
    pub stderr: PathBuf,
    /// Path to the OCI bundle directory.
    pub bundle: PathBuf,
    /// Namespace for containerd
    pub namespace: String,
    /// GRPC address back to main containerd
    pub containerd_address: String,
    /// containerd runtime options config
    pub config: Config,
}

/// Represents a WASI module(s).
/// Instance is a trait that gets implemented by consumers of this library.
/// This trait requires that any type implementing it is `'static`, similar to `std::any::Any`.
/// This means that the type cannot contain a non-`'static` reference.
pub trait Instance: 'static {
    /// The WASI engine type
    type Engine: Send + Sync + Clone;

    /// Create a new instance
    fn new(id: String, cfg: &InstanceConfig) -> Result<Self, Error>
    where
        Self: Sized;

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, Error>;

    /// Send a signal to the instance
    fn kill(&self, signal: u32) -> Result<(), Error>;

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), Error>;

    /// Waits for the instance to finish and returns its exit code
    /// This is a blocking call.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Info"))]
    fn wait(&self) -> (u32, DateTime<Utc>) {
        self.wait_timeout(None).unwrap()
    }

    /// Waits for the instance to finish and returns its exit code
    /// Returns None if the timeout is reached before the instance has finished.
    /// This is a blocking call.
    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)>;
}
