use std::path::Path;

use anyhow::Result;
use tempfile::{TempDir, tempdir};
use trapeze::{Server, ServerHandle, service};

use super::Shim;
use crate::containerd;
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
    _server: ServerHandle,
    verbose: bool,
    containerd: containerd::Client,
}

impl Containerd {
    pub async fn new(client: containerd::Client, verbose: bool) -> Result<Self> {
        let dir = tempdir()?;
        let socket = dir.path().join("containerd.sock.ttrpc");

        let _server = Server::new()
            .register(service!(EventsService: Events))
            .bind(format!("unix://{}", socket.display()))
            .await?;

        Ok(Self {
            dir,
            _server,
            verbose,
            containerd: client,
        })
    }
}

impl crate::traits::Containerd for Containerd {
    type Shim = Shim;
    async fn start_shim(&self, shim: impl AsRef<Path> + Send) -> Result<Shim> {
        Shim::new(self.containerd.clone(), &self.dir, self.verbose, shim).await
    }
}
