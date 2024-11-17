use std::path::Path;

use anyhow::Result;
use containerd_shim_wasm_test_modules::HELLO_WORLD;
use log::info;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
use tempfile::{tempdir_in, TempDir};
use tokio::fs::{canonicalize, create_dir_all, symlink, write};
use trapeze::Client;

use crate::protos::containerd::task::v2::{
    CreateTaskRequest, DeleteRequest, StartRequest, Task as _, WaitRequest,
};
use crate::utils::hash;

pub struct Task {
    id: String,
    dir: TempDir,
    client: Client,
}

impl Task {
    pub(super) async fn new(scratch: impl AsRef<Path>, client: Client) -> Result<Self> {
        let dir = tempdir_in(scratch)?;
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
            HELLO_WORLD.bytes,
        )
        .await?;

        let stdout = canonicalize("/proc/self/fd/1").await?;
        symlink(stdout, dir.path().join("stdout")).await?;

        Ok(Self { id, dir, client })
    }

    pub async fn create(&self, verbose: bool) -> Result<()> {
        let res = self
            .client
            .create(CreateTaskRequest {
                id: self.id.clone(),
                bundle: self.dir.path().to_string_lossy().into_owned(),
                stdout: if !verbose {
                    String::new()
                } else {
                    self.dir
                        .path()
                        .join("stdout")
                        .to_string_lossy()
                        .into_owned()
                },
                ..Default::default()
            })
            .await?;

        info!("create returned {res:?}");

        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        let res = self
            .client
            .start(StartRequest {
                id: self.id.clone(),
                ..Default::default()
            })
            .await?;

        info!("start returned {res:?}");

        Ok(())
    }

    pub async fn wait(&self) -> Result<()> {
        let res = self
            .client
            .wait(WaitRequest {
                id: self.id.clone(),
                ..Default::default()
            })
            .await?;

        info!("wait returned {res:?}");

        Ok(())
    }

    pub async fn delete(&self) -> Result<()> {
        let res = self
            .client
            .delete(DeleteRequest {
                id: self.id.clone(),
                ..Default::default()
            })
            .await?;

        info!("delete returned {res:?}");

        Ok(())
    }
}
