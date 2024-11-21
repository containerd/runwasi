use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::Container;
use libcontainer::signal::Signal;
use libcontainer::syscall::syscall::SyscallType;
use nix::errno::Errno;
use nix::sys::wait::{waitid, Id as WaitID, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use oci_spec::image::Platform;

use crate::container::Engine;
use crate::sandbox::async_utils::AmbientRuntime as _;
use crate::sandbox::instance_utils::{determine_rootdir, get_instance_root, instance_exists};
use crate::sandbox::sync::WaitableCell;
use crate::sandbox::{
    containerd, Error as SandboxError, Instance as SandboxInstance, InstanceConfig, Stdio,
};
use crate::sys::container::executor::Executor;

static DEFAULT_CONTAINER_ROOT_DIR: &str = "/run/containerd";

pub struct Instance<E: Engine> {
    exit_code: WaitableCell<(u32, DateTime<Utc>)>,
    rootdir: PathBuf,
    id: String,
    _phantom: PhantomData<E>,
}

impl<E: Engine> SandboxInstance for Instance<E> {
    type Engine = E;

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn new(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Result<Self, SandboxError> {
        let cfg = cfg.context("missing configuration")?;
        let engine = cfg.get_engine();
        let bundle = cfg.get_bundle().to_path_buf();
        let namespace = cfg.get_namespace();
        let rootdir = Path::new(DEFAULT_CONTAINER_ROOT_DIR).join(E::name());
        let rootdir = determine_rootdir(&bundle, &namespace, rootdir)?;
        let stdio = Stdio::init_from_cfg(cfg)?;

        // check if container is OCI image with wasm layers and attempt to read the module
        let (modules, platform) = containerd::Client::connect(cfg.get_containerd_address().as_str(), &namespace).block_on()?
            .load_modules(&id, &engine)
            .block_on()
            .unwrap_or_else(|e| {
                log::warn!("Error obtaining wasm layers for container {id}.  Will attempt to use files inside container image. Error: {e}");
                (vec![], Platform::default())
            });

        ContainerBuilder::new(id.clone(), SyscallType::Linux)
            .with_executor(Executor::new(engine, stdio, modules, platform))
            .with_root_path(rootdir.clone())?
            .as_init(&bundle)
            .with_systemd(false)
            .build()?;

        Ok(Self {
            id,
            exit_code: WaitableCell::new(),
            rootdir,
            _phantom: Default::default(),
        })
    }

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn start(&self) -> Result<u32, SandboxError> {
        log::info!("starting instance: {}", self.id);
        // make sure we have an exit code by the time we finish (even if there's a panic)
        let guard = self.exit_code.set_guard_with(|| (137, Utc::now()));

        let container_root = get_instance_root(&self.rootdir, &self.id)?;
        let mut container = Container::load(container_root)?;
        let pid = container.pid().context("failed to get pid")?.as_raw();

        container.start()?;

        let exit_code = self.exit_code.clone();
        thread::spawn(move || {
            // move the exit code guard into this thread
            let _guard = guard;

            let status = match waitid(WaitID::Pid(Pid::from_raw(pid)), WaitPidFlag::WEXITED) {
                Ok(WaitStatus::Exited(_, status)) => status,
                Ok(WaitStatus::Signaled(_, sig, _)) => sig as i32,
                Ok(_) => 0,
                Err(Errno::ECHILD) => {
                    log::info!("no child process");
                    0
                }
                Err(e) => {
                    log::error!("waitpid failed: {e}");
                    137
                }
            } as u32;
            let _ = exit_code.set((status, Utc::now()));
        });

        Ok(pid as u32)
    }

    /// Send a signal to the instance
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
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
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn delete(&self) -> Result<(), SandboxError> {
        log::info!("deleting instance: {}", self.id);
        match instance_exists(&self.rootdir, &self.id) {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(err) => {
                log::error!("could not find the container, skipping cleanup: {}", err);
                return Ok(());
            }
        }
        let container_root = get_instance_root(&self.rootdir, &self.id)?;
        match Container::load(container_root) {
            Ok(mut container) => {
                container.delete(true)?;
            }
            Err(err) => {
                log::error!("could not find the container, skipping cleanup: {}", err);
            }
        }
        Ok(())
    }

    /// Waits for the instance to finish and returns its exit code
    /// Returns None if the timeout is reached before the instance has finished.
    /// This is a blocking call.
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip(self, t), level = "Info"))]
    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)> {
        self.exit_code.wait_timeout(t).copied()
    }
}
