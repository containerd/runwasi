use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use nix::NixPath;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder, UserBuilder};
use tempfile::{TempDir, tempdir_in};
use tokio::fs::{create_dir_all, write};
use tokio_async_drop::tokio_async_drop;

use super::task_client::TaskClient;
use crate::containerd;
use crate::protos::containerd::task::v2::*;
use crate::protos::containerd::types::Mount;
use crate::traits::Task as _;
use crate::utils::{RunOnce, make_task_id};

pub struct Task {
    id: String,
    dir: TempDir,
    client: TaskClient,
    mounts: Vec<Mount>,
    deleted: RunOnce,
    unmounted: RunOnce,
}

impl Task {
    pub(super) async fn new<T: Into<String>>(
        containerd: containerd::Client,
        scratch: impl AsRef<Path>,
        image: String,
        args: impl IntoIterator<Item = T>,
        client: TaskClient,
    ) -> Result<Self> {
        let id = make_task_id();
        let mounts = containerd.get_mounts(&id, &image).await?;
        let mounts = map_mounts(mounts);

        let args: Vec<_> = args.into_iter().map(|arg| arg.into()).collect();

        let process = ProcessBuilder::default()
            .user(UserBuilder::default().build().unwrap())
            .args(args)
            .cwd("/")
            .build()?;

        let annotations = [(
            "io.kubernetes.cri.sandbox-id".to_string(),
            format!("sandbox-{}", std::process::id()),
        )];

        let root = RootBuilder::default()
            .path("rootfs")
            .readonly(false)
            .build()?;

        let spec = SpecBuilder::default()
            .version("1.1.0")
            .process(process)
            .annotations(annotations)
            .root(root)
            .build()?;

        let dir = tempdir_in(scratch)?;
        create_dir_all(dir.path().join("rootfs")).await?;
        write(dir.path().join("options.json"), r#"{"root":"rootfs"}"#).await?;
        spec.save(dir.path().join("config.json"))?;

        Ok(Self {
            id,
            dir,
            client,
            mounts,
            deleted: RunOnce::new(),
            unmounted: RunOnce::new(),
        })
    }
}

impl crate::traits::Task for Task {
    async fn create(&self) -> Result<()> {
        let stdout = self.dir.path().join("stdout");
        let stderr = self.dir.path().join("stderr");

        let _ = std::fs::write(&stdout, "");
        let _ = std::fs::write(&stderr, "");

        self.client
            .create(CreateTaskRequest {
                id: self.id.clone(),
                bundle: self.dir.path().to_string_lossy().into_owned(),
                stdout: stdout.to_string_lossy().into_owned(),
                stderr: stderr.to_string_lossy().into_owned(),
                rootfs: self.mounts.clone(),
                ..Default::default()
            })
            .await?;

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        self.client
            .start(StartRequest {
                id: self.id.clone(),
                ..Default::default()
            })
            .await?;
        Ok(())
    }

    async fn wait(&self) -> Result<()> {
        let status = self
            .client
            .wait(WaitRequest {
                id: self.id.clone(),
                ..Default::default()
            })
            .await?
            .exit_status;
        let stdout = std::fs::read_to_string(self.dir.path().join("stdout")).unwrap_or_default();
        let stderr = std::fs::read_to_string(self.dir.path().join("stderr")).unwrap_or_default();
        ensure!(
            status == 0,
            "Exit status {status}, stdout: {stdout:?}, stderr: {stderr:?}"
        );
        Ok(())
    }

    async fn delete(&self) -> Result<()> {
        let res1 = self
            .deleted
            .try_run(async {
                self.client
                    .delete(DeleteRequest {
                        id: self.id.clone(),
                        ..Default::default()
                    })
                    .await?;
                Ok(())
            })
            .await;
        let res2 = self
            .unmounted
            .try_run(async {
                unmount_recursive(self.dir.path().join("rootfs"))?;
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

fn unmount_recursive(root: impl AsRef<Path>) -> Result<()> {
    let root = root.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut mounts = std::fs::read_to_string("/proc/mounts")?
            .lines()
            .filter_map(|m| m.split_whitespace().nth(1).map(|p| p.to_string()))
            .filter_map(|m| {
                let mount = PathBuf::from(m);
                mount.starts_with(&root).then_some(mount)
            })
            .collect::<Vec<_>>();

        mounts.sort_by_key(|p| p.len());

        for mount in mounts.iter().rev() {
            nix::mount::umount(mount)?;
        }

        Ok(())
    });

    Ok(())
}

fn map_mount(m: containerd_client::types::Mount) -> Mount {
    Mount {
        r#type: m.r#type,
        source: m.source,
        target: m.target,
        options: m.options,
    }
}

fn map_mounts(m: impl IntoIterator<Item = containerd_client::types::Mount>) -> Vec<Mount> {
    m.into_iter().map(map_mount).collect()
}
