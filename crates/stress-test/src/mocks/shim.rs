use std::path::Path;

use anyhow::Result;
use log::info;
use oci_spec::runtime::SpecBuilder;
use tempfile::{TempDir, tempdir_in};
use tokio::fs::{canonicalize, symlink};
use tokio::process::Command;
use tokio_async_drop::tokio_async_drop;
use trapeze::Client;

use super::Task;
use crate::containerd;
use crate::protos::containerd::task::v2::{ShutdownRequest, Task as _};

pub struct Shim {
    dir: TempDir,
    client: Client,
    containerd: containerd::Client,
}

impl Shim {
    pub(super) async fn new(
        containerd: containerd::Client,
        scratch: impl AsRef<Path>,
        verbose: bool,
        binary: impl AsRef<Path>,
    ) -> Result<Self> {
        info!("Setting up shim");

        let scratch = scratch.as_ref();

        let socket = scratch.join("containerd.sock.ttrpc");
        let dir = tempdir_in(scratch)?;

        let spec = SpecBuilder::default().build()?;
        spec.save(dir.path().join("config.json"))?;

        if verbose {
            let stderr = canonicalize("/proc/self/fd/2").await?;
            symlink(stderr, dir.path().join("log")).await?;
        } else {
            symlink("/dev/null", dir.path().join("log")).await?;
        }

        info!("Starting shim");

        let pid = std::process::id();
        let output = Command::new(binary.as_ref())
            .args([
                "-namespace",
                &format!("shim-benchmark-{pid}"),
                "-id",
                &format!("shim-benchmark-{pid}"),
                "-address",
                "/run/containerd/containerd.sock",
                "start",
            ])
            .process_group(0)
            .env("TTRPC_ADDRESS", socket)
            .current_dir(dir.path())
            .output()
            .await?;

        let address = String::from_utf8(output.stdout)?.trim().to_owned();

        info!("Connecting to {address}");
        let client = Client::connect(address).await?;

        Ok(Shim {
            dir,
            client,
            containerd,
        })
    }
}

impl crate::traits::Shim for Shim {
    type Task = Task;

    async fn task<T: Into<String>>(
        &self,
        image: impl Into<String>,
        args: impl IntoIterator<Item = T>,
    ) -> Result<Task> {
        Task::new(
            self.containerd.clone(),
            &self.dir,
            image.into(),
            args,
            self.client.clone(),
        )
        .await
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        tokio_async_drop!({
            let _ = self
                .client
                .shutdown(ShutdownRequest {
                    now: true,
                    ..Default::default()
                })
                .await;
        })
    }
}
