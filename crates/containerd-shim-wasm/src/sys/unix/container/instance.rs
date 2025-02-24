use std::marker::PhantomData;
use std::path::Path;
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::syscall::syscall::SyscallType;
use nix::errno::Errno;
use nix::sys::wait::{Id as WaitID, WaitPidFlag, WaitStatus, waitid};
use nix::unistd::Pid;
use oci_spec::image::Platform;

use super::container::Container;
use crate::container::Engine;
use crate::sandbox::async_utils::AmbientRuntime as _;
use crate::sandbox::instance_utils::determine_rootdir;
use crate::sandbox::sync::WaitableCell;
use crate::sandbox::{
    Error as SandboxError, Instance as SandboxInstance, InstanceConfig, containerd,
};
use crate::sys::container::executor::Executor;
use crate::sys::stdio::open;

const DEFAULT_CONTAINER_ROOT_DIR: &str = "/run/containerd";

pub struct Instance<E: Engine> {
    exit_code: WaitableCell<(u32, DateTime<Utc>)>,
    container: Container,
    id: String,
    _phantom: PhantomData<E>,
}

impl<E: Engine + Default> SandboxInstance for Instance<E> {
    type Engine = E;

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Info"))]
    fn new(id: String, cfg: &InstanceConfig) -> Result<Self, SandboxError> {
        // check if container is OCI image with wasm layers and attempt to read the module
        let (modules, platform) = containerd::Client::connect(cfg.get_containerd_address(), &cfg.get_namespace()).block_on()?
            .load_modules(&id, &E::default())
            .block_on()
            .unwrap_or_else(|e| {
                log::warn!("Error obtaining wasm layers for container {id}.  Will attempt to use files inside container image. Error: {e}");
                (vec![], Platform::default())
            });

        let container = Container::build(
            |(id, cfg, modules, platform)| {
                let namespace = cfg.get_namespace();

                let bundle = cfg.get_bundle().to_path_buf();
                let rootdir = Path::new(DEFAULT_CONTAINER_ROOT_DIR).join(E::name());
                let rootdir = determine_rootdir(&bundle, &namespace, rootdir)?;
                let engine = E::default();

                let mut builder = ContainerBuilder::new(id.clone(), SyscallType::Linux)
                    .with_executor(Executor::new(engine, modules, platform))
                    .with_root_path(rootdir.clone())?;

                if let Ok(f) = open(cfg.get_stdin()) {
                    builder = builder.with_stdin(f);
                }
                if let Ok(f) = open(cfg.get_stdout()) {
                    builder = builder.with_stdout(f);
                }
                if let Ok(f) = open(cfg.get_stderr()) {
                    builder = builder.with_stderr(f);
                }
                let systemd_cgroup = cfg.get_systemd_cgroup();

                let container = builder
                    .as_init(&bundle)
                    .as_sibling(true)
                    .with_systemd(systemd_cgroup)
                    .build()?;

                Ok(container)
            },
            (id.clone(), cfg.clone(), modules, platform),
        )?;

        Ok(Self {
            id,
            exit_code: WaitableCell::new(),
            container,
            _phantom: Default::default(),
        })
    }

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Info"))]
    fn start(&self) -> Result<u32, SandboxError> {
        log::info!("starting instance: {}", self.id);
        // make sure we have an exit code by the time we finish (even if there's a panic)
        let guard = self.exit_code.clone().set_guard_with(|| (137, Utc::now()));

        let pid = self.container.pid()?;
        self.container.start()?;

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
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Info"))]
    fn kill(&self, signal: u32) -> Result<(), SandboxError> {
        log::info!("sending signal {signal} to instance: {}", self.id);
        self.container.kill(signal)?;
        Ok(())
    }

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Info"))]
    fn delete(&self) -> Result<(), SandboxError> {
        log::info!("deleting instance: {}", self.id);
        self.container.delete()?;
        Ok(())
    }

    /// Waits for the instance to finish and returns its exit code
    /// Returns None if the timeout is reached before the instance has finished.
    /// This is a blocking call.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, t), level = "Info")
    )]
    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)> {
        self.exit_code.wait_timeout(t).copied()
    }
}
