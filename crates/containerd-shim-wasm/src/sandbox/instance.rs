//! Abstractions for running/managing a wasm/wasi instance.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::error::Error;
use super::sync::WaitableCell;
use crate::sys::signals::*;

/// Generic options builder for creating a wasm instance.
/// This is passed to the `Instance::new` method.
#[derive(Clone)]
pub struct InstanceConfig<Engine: Send + Sync + Clone> {
    /// The WASI engine to use.
    /// This should be cheap to clone.
    engine: Engine,
    /// Optional stdin named pipe path.
    stdin: PathBuf,
    /// Optional stdout named pipe path.
    stdout: PathBuf,
    /// Optional stderr named pipe path.
    stderr: PathBuf,
    /// Path to the OCI bundle directory.
    bundle: PathBuf,
    /// Namespace for containerd
    namespace: String,
    // /// GRPC address back to main containerd
    containerd_address: String,
}

impl<Engine: Send + Sync + Clone> InstanceConfig<Engine> {
    pub fn new(
        engine: Engine,
        namespace: impl AsRef<str>,
        containerd_address: impl AsRef<str>,
    ) -> Self {
        let namespace = namespace.as_ref().to_string();
        let containerd_address = containerd_address.as_ref().to_string();
        Self {
            engine,
            namespace,
            containerd_address,
            stdin: PathBuf::default(),
            stdout: PathBuf::default(),
            stderr: PathBuf::default(),
            bundle: PathBuf::default(),
        }
    }

    /// set the stdin path for the instance
    pub fn set_stdin(&mut self, stdin: impl AsRef<Path>) -> &mut Self {
        self.stdin = stdin.as_ref().to_path_buf();
        self
    }

    /// get the stdin path for the instance
    pub fn get_stdin(&self) -> &Path {
        &self.stdin
    }

    /// set the stdout path for the instance
    pub fn set_stdout(&mut self, stdout: impl AsRef<Path>) -> &mut Self {
        self.stdout = stdout.as_ref().to_path_buf();
        self
    }

    /// get the stdout path for the instance
    pub fn get_stdout(&self) -> &Path {
        &self.stdout
    }

    /// set the stderr path for the instance
    pub fn set_stderr(&mut self, stderr: impl AsRef<Path>) -> &mut Self {
        self.stderr = stderr.as_ref().to_path_buf();
        self
    }

    /// get the stderr path for the instance
    pub fn get_stderr(&self) -> &Path {
        &self.stderr
    }

    /// set the OCI bundle path for the instance
    pub fn set_bundle(&mut self, bundle: impl AsRef<Path>) -> &mut Self {
        self.bundle = bundle.as_ref().to_path_buf();
        self
    }

    /// get the OCI bundle path for the instance
    pub fn get_bundle(&self) -> &Path {
        &self.bundle
    }

    /// get the wasm engine for the instance
    pub fn get_engine(&self) -> Engine {
        self.engine.clone()
    }

    /// get the namespace for the instance
    pub fn get_namespace(&self) -> String {
        self.namespace.clone()
    }

    /// get the containerd address for the instance
    pub fn get_containerd_address(&self) -> String {
        self.containerd_address.clone()
    }
}

/// Represents a WASI module(s).
/// Instance is a trait that gets implemented by consumers of this library.
/// This trait requires that any type implementing it is `'static`, similar to `std::any::Any`.
/// This means that the type cannot contain a non-`'static` reference.
pub trait Instance: 'static {
    /// The WASI engine type
    type Engine: Send + Sync + Clone;

    /// Create a new instance
    fn new(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Result<Self, Error>
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
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), parent = tracing::Span::current(), level = "Info"))]
    fn wait(&self) -> (u32, DateTime<Utc>) {
        self.wait_timeout(None).unwrap()
    }

    /// Waits for the instance to finish and retunrs its exit code
    /// Returns None if the timeout is reached before the instance has finished.
    /// This is a blocking call.
    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)>;
}

/// This is used for the "pause" container with cri and is a no-op instance implementation.
pub struct Nop {
    /// Since we are faking the container, we need to keep track of the "exit" code/time
    /// We'll just mark it as exited when kill is called.
    exit_code: WaitableCell<(u32, DateTime<Utc>)>,
}

impl Instance for Nop {
    type Engine = ();
    fn new(_id: String, _cfg: Option<&InstanceConfig<Self::Engine>>) -> Result<Self, Error> {
        Ok(Nop {
            exit_code: WaitableCell::new(),
        })
    }
    fn start(&self) -> Result<u32, Error> {
        Ok(std::process::id())
    }
    fn kill(&self, signal: u32) -> Result<(), Error> {
        let code = match signal as i32 {
            SIGKILL => 137,
            SIGINT | SIGTERM => 0,
            s => {
                return Err(Error::InvalidArgument(format!("unsupported signal: {}", s)));
            }
        };

        let _ = self.exit_code.set((code, Utc::now()));

        Ok(())
    }
    fn delete(&self) -> Result<(), Error> {
        Ok(())
    }
    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)> {
        self.exit_code.wait_timeout(t).copied()
    }
}

#[cfg(test)]
mod noptests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_nop_kill_sigkill() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None)?;

        nop.kill(SIGKILL as u32)?;

        let ec = nop.wait_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 137);
        Ok(())
    }

    #[test]
    fn test_nop_kill_sigterm() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None)?;

        nop.kill(SIGTERM as u32)?;

        let ec = nop.wait_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 0);
        Ok(())
    }

    #[test]
    fn test_nop_kill_sigint() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None)?;

        nop.kill(SIGINT as u32)?;

        let ec = nop.wait_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 0);
        Ok(())
    }

    #[test]
    fn test_nop_delete_after_create() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None)?;
        nop.delete()?;
        Ok(())
    }
}
