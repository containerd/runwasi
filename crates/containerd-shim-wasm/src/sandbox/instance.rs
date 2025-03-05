//! Abstractions for running/managing a wasm/wasi instance.

use std::path::PathBuf;

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
#[trait_variant::make(Send)]
pub trait Instance: 'static {
    /// Create a new instance
    async fn new(id: String, cfg: &InstanceConfig) -> Result<Self, Error>
    where
        Self: Sized;

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    async fn start(&self) -> Result<u32, Error>;

    /// Send a signal to the instance
    async fn kill(&self, signal: u32) -> Result<(), Error>;

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    async fn delete(&self) -> Result<(), Error>;

    /// Waits for the instance to finish and returns its exit code
    /// This is an async call.
    async fn wait(&self) -> (u32, DateTime<Utc>);
}
