use std::path::PathBuf;

use anyhow::Result;
use containerd_client::types::Mount;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, Spec, SpecBuilder, UserBuilder};
use tokio::fs::canonicalize;
use tokio_async_drop::tokio_async_drop;

use super::Client;
use crate::traits::Task as _;
use crate::utils::{make_task_id, RunOnce};

pub struct Task {
    containerd: Client,
    runtime: String,
    id: String,
    image: String,
    mounts: Vec<Mount>,
    spec: Spec,
    task_deleted: RunOnce,
    container_deleted: RunOnce,
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
        })
    }
}

impl crate::traits::Task for Task {
    async fn create(&self, verbose: bool) -> Result<()> {
        let stdout = if !verbose {
            PathBuf::new()
        } else if let Ok(stdout) = canonicalize("/proc/self/fd/1").await {
            stdout
        } else {
            PathBuf::new()
        };

        let stdout = stdout.to_string_lossy().into_owned();

        self.containerd
            .create_container(&self.id, &self.image, &self.runtime, self.spec.clone())
            .await?;

        self.containerd
            .create_task(&self.id, &self.mounts[..], stdout)
            .await?;

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        self.containerd.start_task(&self.id).await
    }

    async fn wait(&self) -> Result<()> {
        self.containerd.wait_task(&self.id).await
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
