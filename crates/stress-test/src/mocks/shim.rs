use std::path::Path;

use anyhow::Result;
use log::info;
use oci_spec::runtime::SpecBuilder;
use tempfile::{tempdir_in, TempDir};
use tokio::fs::{canonicalize, symlink};
use tokio::process::Command;
use trapeze::Client;

use super::Task;
use crate::utils::hash;

pub struct Shim {
    pub(super) dir: TempDir,
    pub(super) client: Client,
}

impl Shim {
    pub(super) async fn new(
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

        let hash = hash(dir.path());
        let output = Command::new(binary.as_ref())
            .args([
                "-namespace",
                &format!("shim-benchmark-{hash}"),
                "-id",
                &format!("shim-benchmark-{hash}"),
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

        Ok(Shim { dir, client })
    }

    pub async fn task(&self) -> Result<Task> {
        Task::new(&self.dir, self.client.clone()).await
    }
}
