//! Abstractions for running/managing a wasm/wasi instance that uses youki's libcontainer library.

use std::path::PathBuf;
use std::thread;

use anyhow::Context;
use chrono::Utc;
use libcontainer::container::{Container, ContainerStatus};
use libcontainer::signal::Signal;
use log::error;
use nix::errno::Errno;
use nix::sys::wait::{waitid, Id as WaitID, WaitPidFlag, WaitStatus};

use crate::sandbox::error::Error;
use crate::sandbox::instance::{ExitCode, Wait};
use crate::sandbox::instance_utils::{get_instance_root, instance_exists};
use crate::sandbox::{Instance, InstanceConfig};
use crate::sys::signals::{SIGINT, SIGKILL};

/// LibcontainerInstance is a trait that gets implemented by a WASI runtime that
/// uses youki's libcontainer library as the container runtime.
/// It provides default implementations for some of the Instance trait methods.
/// The implementor of this trait is expected to implement the
/// * `new_libcontainer()`
/// * `get_exit_code()`
/// * `get_id()`
/// * `get_root_dir()`
/// * `build_container()`
/// methods.
pub trait LibcontainerInstance {
    /// The WASI engine type
    type Engine: Send + Sync + Clone;

    /// Create a new instance
    fn new_libcontainer(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Self;

    /// Get the exit code of the instance
    fn get_exit_code(&self) -> ExitCode;

    /// Get the ID of the instance
    fn get_id(&self) -> String;

    /// Get the root directory of the instance
    fn get_root_dir(&self) -> Result<PathBuf, Error>;

    /// Build the container
    fn build_container(&self) -> Result<Container, Error>;
}

/// Default implementation of the Instance trait for YoukiInstance
/// This implementation uses the libcontainer library to create and start
/// the container.
impl<T: LibcontainerInstance> Instance for T {
    type Engine = T::Engine;

    fn new(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Self {
        Self::new_libcontainer(id, cfg)
    }

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, Error> {
        let id = self.get_id();
        log::info!("starting instance: {}", id);

        let mut container = self.build_container()?;
        let code = self.get_exit_code();
        let pid = container.pid().context("failed to get pid")?;

        container
            .start()
            .map_err(|err| Error::Any(anyhow::anyhow!("failed to start container: {}", err)))?;

        thread::spawn(move || {
            let (lock, cvar) = &*code;

            let status = match waitid(WaitID::Pid(pid), WaitPidFlag::WEXITED) {
                Ok(WaitStatus::Exited(_, status)) => status,
                Ok(WaitStatus::Signaled(_, sig, _)) => sig as i32,
                Ok(_) => 0,
                Err(e) => {
                    if e == Errno::ECHILD {
                        log::info!("no child process");
                        0
                    } else {
                        panic!("waitpid failed: {}", e);
                    }
                }
            } as u32;
            let mut ec = lock.lock().unwrap();
            *ec = Some((status, Utc::now()));
            drop(ec);
            cvar.notify_all();
        });

        Ok(pid.as_raw() as u32)
    }

    /// Send a signal to the instance
    fn kill(&self, signal: u32) -> Result<(), Error> {
        let id = self.get_id();
        let root_dir = self.get_root_dir()?;
        log::info!("killing instance: {}", id.clone());
        if signal as i32 != SIGKILL && signal as i32 != SIGINT {
            return Err(Error::InvalidArgument(
                "only SIGKILL and SIGINT are supported".to_string(),
            ));
        }
        let signal = Signal::try_from(signal as i32)
            .map_err(|err| Error::InvalidArgument(format!("invalid signal number: {}", err)))?;
        let container_root = get_instance_root(root_dir, id.as_str())?;
        let mut container = Container::load(container_root).with_context(|| {
            format!("could not load state for container {id}", id = id.as_str())
        })?;

        match container.kill(signal, true) {
            Ok(_) => Ok(()),
            Err(e) => {
                if container.status() == ContainerStatus::Stopped {
                    return Err(Error::Others("container not running".into()));
                }
                Err(Error::Others(e.to_string()))
            }
        }
    }

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), Error> {
        let id = self.get_id();
        let root_dir = self.get_root_dir()?;
        log::info!("deleting instance: {}", id.clone());
        match instance_exists(&root_dir, id.as_str()) {
            Ok(exists) => {
                if !exists {
                    return Ok(());
                }
            }
            Err(err) => {
                error!("could not find the container, skipping cleanup: {}", err);
                return Ok(());
            }
        }
        let container_root = get_instance_root(&root_dir, id.as_str())?;
        let container = Container::load(container_root).with_context(|| {
            format!(
                "could not load state for container {id}",
                id = id.clone().as_str()
            )
        });
        match container {
            Ok(mut container) => container.delete(true).map_err(|err| {
                Error::Any(anyhow::anyhow!(
                    "failed to delete container {}: {}",
                    id,
                    err
                ))
            })?,
            Err(err) => {
                error!("could not find the container, skipping cleanup: {}", err);
                return Ok(());
            }
        }
        Ok(())
    }

    /// Set up waiting for the instance to exit
    /// The Wait struct is used to send the exit code and time back to the
    /// caller. The recipient is expected to call function
    /// set_up_exit_code_wait() implemented by Wait to set up exit code
    /// processing. Note that the "wait" function doesn't block, but
    /// it sets up the waiting channel.
    fn wait(&self, waiter: &Wait) -> Result<(), Error> {
        let id = self.get_id();
        let exit_code = self.get_exit_code();
        log::info!("waiting for instance: {}", id);
        let code = exit_code;
        waiter.set_up_exit_code_wait(code)
    }
}
