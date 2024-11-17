use std::path::Path;

use anyhow::Result;
use tempfile::{tempdir, TempDir};
use trapeze::{service, Server, ServerHandle};

use super::Shim;
use crate::protos::containerd::services::events::ttrpc::v1::{Events, ForwardRequest};

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
        Shim::new(&self.dir, self.verbose, shim).await
    }

    pub async fn shutdown(self) -> Result<()> {
        self.server.terminate();
        Ok(self.server.await?)
    }
}
