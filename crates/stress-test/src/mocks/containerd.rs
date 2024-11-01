use std::path::Path;

use anyhow::Result;
use log::info;
use oci_spec::runtime::SpecBuilder;
use tempfile::{tempdir, tempdir_in, TempDir};
use tokio::fs::{canonicalize, symlink};
use tokio::process::Command;
use trapeze::{service, Client, Server, ServerHandle};

use super::Shim;
use crate::protos::containerd::services::events::ttrpc::v1::{Events, ForwardRequest};
use crate::utils::hash;

struct EventsService;

impl Events for EventsService {
    async fn forward(&self, forward_request: ForwardRequest) -> trapeze::Result<()> {
        log::info!("forward_request: {forward_request:?}");
        Ok(())
    }
}

pub struct Containerd {
    dir: TempDir,
    server: ServerHandle,
    verbose: bool,
}

impl Containerd {
    pub async fn new(verbose: bool) -> Result<Self> {
        let dir = tempdir()?;
        let socket = dir.path().join("containerd.sock.ttrpc");

        let server = Server::new()
            .register(service!(EventsService: Events))
            .bind(format!("unix://{}", socket.display()))
            .await?;

        Ok(Self {
            dir,
            server,
            verbose,
        })
    }

    pub async fn start_shim(&self, shim: impl AsRef<Path>) -> Result<Shim> {
        info!("Setting up shim");

        let socket = self.dir.path().join("containerd.sock.ttrpc");
        let dir = tempdir_in(self.dir.path())?;

        let spec = SpecBuilder::default().build()?;
        spec.save(dir.path().join("config.json"))?;

        if self.verbose {
            let stderr = canonicalize("/proc/self/fd/2").await?;
            symlink(stderr, dir.path().join("log")).await?;
        } else {
            symlink("/dev/null", dir.path().join("log")).await?;
        }

        info!("Starting shim");

        let hash = hash(dir.path());
        let output = Command::new(shim.as_ref())
            .args([
                "-namespace",
                &format!("shim-benchmark-{hash}"),
                "-id",
                &format!("shim-benchmark-{hash}"),
                "-address",
                "/run/containerd/containerd.sock",
                "start",
            ])
            .env("TTRPC_ADDRESS", socket)
            .current_dir(dir.path())
            .output()
            .await?;

        let address = String::from_utf8(output.stdout)?.trim().to_owned();

        info!("Connecting to {address}");
        let client = Client::connect(address).await?;

        Ok(Shim { dir, client })
    }

    pub async fn shutdown(self) -> Result<()> {
        self.server.shutdown();
        Ok(self.server.await?)
    }
}
