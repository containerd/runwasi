use anyhow::{ensure, Result};
use containerd_client::types::Mount;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, Spec, SpecBuilder, UserBuilder};
use tempfile::{tempdir, TempDir};
use tokio_async_drop::tokio_async_drop;

use super::Client;
use crate::traits::Task as _;
use crate::utils::{RunOnce, make_task_id};

pub struct Task {
    containerd: Client,
    runtime: String,
    id: String,
    image: String,
    mounts: Vec<Mount>,
    spec: Spec,
    task_deleted: RunOnce,
    container_deleted: RunOnce,
    dir: TempDir,
}

impl Task {
    pub(super) async fn new<T: Into<String>>(
        containerd: Client,
        runtime: impl Into<String>,
        image: impl Into<String>,
        args: impl IntoIterator<Item = T>,
    ) -> Result<Self> {
        let image = image.into();
        let runtime = runtime.into();

        let id = make_task_id();
        let entrypoint = containerd.entrypoint(&image).await?;
        let mounts = containerd.get_mounts(&id, &image).await?;

        let mut args: Vec<_> = args.into_iter().map(|arg| arg.into()).collect();
        if args.is_empty() {
            args = entrypoint;
        } else if let Some(argv0) = entrypoint.into_iter().next() {
            args.insert(0, argv0);
        }

        let process = ProcessBuilder::default()
            .user(UserBuilder::default().build().unwrap())
            .args(args)
            .cwd("/")
            .build()?;

        let annotations = [(
            "io.kubernetes.cri.sandbox-id".to_string(),
            format!("sandbox-{}", std::process::id()),
        )];

        let root = RootBuilder::default().path("rootfs").build()?;

        let spec = SpecBuilder::default()
            .version("1.1.0")
            .process(process)
            .annotations(annotations)
            .root(root)
            .build()?;

        Ok(Self {
            containerd,
            runtime,
            id,
            image,
            mounts,
            spec,
            task_deleted: RunOnce::new(),
            container_deleted: RunOnce::new(),
            dir: tempdir()?,
        })
    }
}

impl crate::traits::Task for Task {
    async fn create(&self) -> Result<()> {
        let stdout = self.dir.path().join("stdout");
        let stderr = self.dir.path().join("stderr");

        let _ = std::fs::write(&stdout, "");
        let _ = std::fs::write(&stderr, "");

        let stdout = stdout.to_string_lossy().into_owned();
        let stderr = stderr.to_string_lossy().into_owned();

        self.containerd
            .create_container(&self.id, &self.image, &self.runtime, self.spec.clone())
            .await?;

        self.containerd
            .create_task(&self.id, &self.mounts[..], stdout, stderr)
            .await?;

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        self.containerd.start_task(&self.id).await
    }

    async fn wait(&self) -> Result<()> {
        let status = self.containerd.wait_task(&self.id).await?;
        let stdout = std::fs::read_to_string(self.dir.path().join("stdout")).unwrap_or_default();
        let stderr = std::fs::read_to_string(self.dir.path().join("stderr")).unwrap_or_default();
        ensure!(status == 0, "Exit status {status}, stdout: {stdout:?}, stderr: {stderr:?}");
        Ok(())
    }

    async fn delete(&self) -> Result<()> {
        let res1 = self
            .task_deleted
            .try_run(async {
                let _ = self.containerd.kill_task(&self.id).await;
                self.containerd.delete_task(&self.id).await?;
                Ok(())
            })
            .await;
        let res2 = self
            .container_deleted
            .try_run(async {
                self.containerd.delete_container(&self.id).await?;
                Ok(())
            })
            .await;
        res1.and(res2)
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        tokio_async_drop!({
            let _ = self.delete().await;
        })
    }
}
