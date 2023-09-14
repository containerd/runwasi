use std::path::{Path, PathBuf};
use std::thread;

use anyhow::Context;
use chrono::Utc;
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::Container;
use libcontainer::signal::Signal;
use libcontainer::syscall::syscall::SyscallType;
use nix::errno::Errno;
use nix::sys::wait::{waitid, Id as WaitID, WaitPidFlag, WaitStatus};

use crate::container::Engine;
use crate::sandbox::instance::{ExitCode, Wait};
use crate::sandbox::instance_utils::{determine_rootdir, get_instance_root, instance_exists};
use crate::sandbox::{Error as SandboxError, Instance as SandboxInstance, InstanceConfig, Stdio};
use crate::sys::container::executor::Executor;

static DEFAULT_CONTAINER_ROOT_DIR: &str = "/run/containerd";

pub struct Instance<E: Engine> {
    engine: E,
    exit_code: ExitCode,
    stdio: Stdio,
    bundle: PathBuf,
    rootdir: PathBuf,
    id: String,
}

impl<E: Engine> SandboxInstance for Instance<E> {
    type Engine = E;

    fn new(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Self {
        let cfg = cfg.unwrap();
        let engine = cfg.get_engine();
        let bundle = cfg.get_bundle().unwrap_or_default().into();
        let namespace = cfg.get_namespace();
        let rootdir = Path::new(DEFAULT_CONTAINER_ROOT_DIR).join(E::name());
        let rootdir = determine_rootdir(&bundle, &namespace, rootdir).unwrap();
        let stdio = Stdio::init_from_cfg(cfg).expect("failed to open stdio");
        Self {
            id,
            exit_code: ExitCode::default(),
            engine,
            stdio,
            bundle,
            rootdir,
        }
    }

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, SandboxError> {
        log::info!("starting instance: {}", self.id);
        let mut container = ContainerBuilder::new(self.id.clone(), SyscallType::Linux)
            .with_executor(Executor::new(self.engine.clone(), self.stdio.take()))
            .with_root_path(self.rootdir.clone())?
            .as_init(&self.bundle)
            .with_systemd(false)
            .build()?;

        let pid = container.pid().context("failed to get pid")?;

        container.start().map_err(|err| {
            SandboxError::Any(anyhow::anyhow!("failed to start container: {}", err))
        })?;

        let exit_code = self.exit_code.clone();
        thread::spawn(move || {
            let (lock, cvar) = &*exit_code;

            let status = match waitid(WaitID::Pid(pid), WaitPidFlag::WEXITED) {
                Ok(WaitStatus::Exited(_, status)) => status,
                Ok(WaitStatus::Signaled(_, sig, _)) => sig as i32,
                Ok(_) => 0,
                Err(Errno::ECHILD) => {
                    log::info!("no child process");
                    0
                }
                Err(e) => panic!("waitpid failed: {e}"),
            } as u32;
            let mut ec = lock.lock().unwrap();
            *ec = Some((status, Utc::now()));
            drop(ec);
            cvar.notify_all();
        });

        Ok(pid.as_raw() as u32)
    }

    /// Send a signal to the instance
    fn kill(&self, signal: u32) -> Result<(), SandboxError> {
        log::info!("sending signal {signal} to instance: {}", self.id);
        let signal = Signal::try_from(signal as i32).map_err(|err| {
            SandboxError::InvalidArgument(format!("invalid signal number: {}", err))
        })?;
        let container_root = get_instance_root(&self.rootdir, &self.id)?;
        let mut container = Container::load(container_root)
            .with_context(|| format!("could not load state for container {}", self.id))?;

        container.kill(signal, true)?;

        Ok(())
    }

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), SandboxError> {
        log::info!("deleting instance: {}", self.id);
        match instance_exists(&self.rootdir, &self.id) {
            Ok(exists) => {
                if !exists {
                    return Ok(());
                }
            }
            Err(err) => {
                log::error!("could not find the container, skipping cleanup: {}", err);
                return Ok(());
            }
        }
        let container_root = get_instance_root(&self.rootdir, &self.id)?;
        let container = Container::load(container_root)
            .with_context(|| format!("could not load state for container {}", self.id));
        match container {
            Ok(mut container) => container.delete(true).map_err(|err| {
                SandboxError::Any(anyhow::anyhow!(
                    "failed to delete container {}: {}",
                    &self.id,
                    err
                ))
            })?,
            Err(err) => {
                log::error!("could not find the container, skipping cleanup: {}", err);
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
    fn wait(&self, waiter: &Wait) -> Result<(), SandboxError> {
        log::info!("waiting for instance: {}", self.id);
        waiter.set_up_exit_code_wait(self.exit_code.clone())
    }
}
