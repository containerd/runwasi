use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::fs::{remove_file, symlink};
use tokio_async_drop::tokio_async_drop;

use super::{Client, Task};
use crate::containerd;

pub struct Shim {
    link: PathBuf,
    runtime: String,
    containerd: Client,
}

impl Shim {
    pub(super) async fn new(
        containerd: containerd::Client,
        binary: impl AsRef<Path>,
    ) -> Result<Self> {
        let pid = std::process::id();
        let runtime = format!("io.containerd.runwasi{pid}.v1");
        let link = format!("/usr/local/bin/containerd-shim-runwasi{pid}-v1");
        let link = PathBuf::from(link);
        symlink(binary.as_ref().canonicalize()?, &link).await?;

        Ok(Self {
            containerd,
            link,
            runtime,
        })
    }
}

impl Drop for Shim {
    fn drop(&mut self) {
        tokio_async_drop!({
            let _ = remove_file(&self.link).await;
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
        Task::new(self.containerd.clone(), &self.runtime, image, args).await
    }
}
