use std::marker::PhantomData;

use chrono::{DateTime, Utc};
use containerd_client::tonic::async_trait;
use containerd_shimkit::sandbox::sync::WaitableCell;
use containerd_shimkit::sandbox::{
    Error as SandboxError, Instance as SandboxInstance, InstanceConfig,
};
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::syscall::syscall::SyscallType;
use nix::sys::wait::WaitStatus;
use oci_spec::image::Platform;
use tokio::sync::OnceCell;

use super::container::Container;
use crate::container::{Compiler, Shim};
use crate::sandbox::{WasmLayer, containerd};
use crate::sys::container::executor::Executor;
use crate::sys::pid_fd::PidFd;

pub struct Instance<S: Shim> {
    exit_code: WaitableCell<(u32, DateTime<Utc>)>,
    container: Container,
    id: String,
    _phantom: PhantomData<S>,
}

#[async_trait]
trait OciClient {
    async fn load_modules(&self, id: &str) -> Result<(Vec<WasmLayer>, Platform), SandboxError>;
}

struct EngineOciClient<P: Compiler> {
    client: containerd::Client,
    precompiler: Option<P>,
    supported_layer_types: &'static [&'static str],
    name: &'static str,
}

#[async_trait]
impl<P: Compiler> OciClient for EngineOciClient<P> {
    async fn load_modules(&self, id: &str) -> Result<(Vec<WasmLayer>, Platform), SandboxError> {
        self.client
            .load_modules(
                id,
                self.name,
                self.supported_layer_types,
                self.precompiler.as_ref(),
            )
            .await
    }
}

static OCI_CLIENT: OnceCell<Box<dyn OciClient + Send + Sync + 'static>> = OnceCell::const_new();

impl<S: Shim + Default> SandboxInstance for Instance<S> {
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Info"))]
    async fn new(id: String, cfg: &InstanceConfig) -> Result<Self, SandboxError> {
        let oci_client = OCI_CLIENT
            .get_or_try_init(|| async {
                let client =
                    containerd::Client::connect(&cfg.containerd_address, &cfg.namespace).await?;
                let precompiler = S::compiler().await;
                let supported_layer_types = S::supported_layers_types();
                let name = S::name();
                Result::<_, SandboxError>::Ok(Box::new(EngineOciClient {
                    client,
                    precompiler,
                    supported_layer_types,
                    name,
                }) as _)
            })
            .await?;

        // check if container is OCI image with wasm layers and attempt to read the module
        let (modules, platform) = oci_client
            .load_modules(&id)
            .await
            .unwrap_or_else(|e| {
                log::warn!("Error obtaining wasm layers for container {id}.  Will attempt to use files inside container image. Error: {e}");
                (vec![], Platform::default())
            });

        let container = Container::build(
            |(id, cfg, modules, platform)| {
                let rootdir = cfg.determine_rootdir(S::name())?;

                let mut builder = ContainerBuilder::new(id.clone(), SyscallType::Linux)
                    .with_executor(Executor::<S::Sandbox>::new(modules, platform, id))
                    .with_root_path(rootdir.clone())?;

                if let Ok(f) = cfg.open_stdin() {
                    builder = builder.with_stdin(f);
                }
                if let Ok(f) = cfg.open_stdout() {
                    builder = builder.with_stdout(f);
                }
                if let Ok(f) = cfg.open_stderr() {
                    builder = builder.with_stderr(f);
                }

                let container = builder
                    .as_init(&cfg.bundle)
                    .as_sibling(true)
                    .with_systemd(cfg.config.systemd_cgroup)
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
    async fn start(&self) -> Result<u32, SandboxError> {
        log::info!("starting instance: {}", self.id);
        // make sure we have an exit code by the time we finish (even if there's a panic)
        let guard = self.exit_code.clone().set_guard_with(|| (137, Utc::now()));

        let pid = self.container.pid()?;

        // Use a pidfd FD so that we can wait for the process to exit asynchronously.
        // This should be created BEFORE calling container.start() to ensure we never
        // miss the SIGCHLD event.
        let pidfd = PidFd::new(pid)?;

        self.container.start()?;

        let exit_code = self.exit_code.clone();
        tokio::spawn(async move {
            // move the exit code guard into this task
            let _guard = guard;

            let status = match pidfd.wait().await {
                Ok(WaitStatus::Exited(_, status)) => status,
                Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
                Ok(res) => {
                    log::error!("waitpid unexpected result: {res:?}");
                    137
                }
                Err(e) => {
                    log::error!("waitpid failed: {e}");
                    137
                }
            } as u32;
            let _ = exit_code.set((status, Utc::now()));
        });

        Ok(pid as _)
    }

    /// Send a signal to the instance
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Info"))]
    async fn kill(&self, signal: u32) -> Result<(), SandboxError> {
        log::info!("sending signal {signal} to instance: {}", self.id);
        self.container.kill(signal)?;
        Ok(())
    }

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Info"))]
    async fn delete(&self) -> Result<(), SandboxError> {
        log::info!("deleting instance: {}", self.id);
        self.container.delete()?;
        Ok(())
    }

    /// Waits for the instance to finish and returns its exit code
    /// Returns None if the timeout is reached before the instance has finished.
    /// This is an async call.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Info"))]
    async fn wait(&self) -> (u32, DateTime<Utc>) {
        *self.exit_code.wait().await
    }
}
