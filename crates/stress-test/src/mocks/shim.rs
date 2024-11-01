use anyhow::Result;
use containerd_shim_wasm_test_modules as modules;
use log::info;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
use tempfile::{tempdir_in, TempDir};
use tokio::fs::{canonicalize, create_dir_all, symlink, write};
use trapeze::Client;

use super::Task;
use crate::protos::containerd::task::v2::Task as _;
use crate::utils::hash;

pub struct Shim {
    pub(super) dir: TempDir,
    pub(super) client: Client,
}

impl Shim {
    pub async fn task(&self) -> Result<Task> {
        let dir = tempdir_in(&self.dir)?;
        let id = hash(dir.path());
        let id = format!("shim-benchmark-task-{id}");

        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args([String::from("/hello.wasm")])
                    .build()?,
            )
            .build()?;
        spec.save(dir.path().join("config.json"))?;

        let options = format!("{{\"root\":{:?}}}", dir.path().join("rootfs"));
        write(dir.path().join("options.json"), options).await?;

        create_dir_all(dir.path().join("rootfs")).await?;

        write(
            dir.path().join("rootfs").join("hello.wasm"),
            modules::HELLO_WORLD.bytes,
        )
        .await?;

        let stdout = canonicalize("/proc/self/fd/1").await?;
        symlink(stdout, dir.path().join("stdout")).await?;

        Ok(Task {
            id: id.into(),
            dir,
            client: self.client.clone(),
        })
    }

    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down");

        self.client.shutdown(Default::default()).await?;
        Ok(())
    }
}
