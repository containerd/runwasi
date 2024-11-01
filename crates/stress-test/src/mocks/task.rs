use anyhow::Result;
use log::info;
use tempfile::TempDir;
use trapeze::Client;

use crate::protos::containerd::task::v2::{
    CreateTaskRequest, DeleteRequest, StartRequest, Task as _, WaitRequest,
};

pub struct Task {
    pub(super) id: String,
    pub(super) dir: TempDir,
    pub(super) client: Client,
}

impl Task {
    pub async fn create(&self) -> Result<()> {
        let res = self
            .client
            .create(CreateTaskRequest {
                id: self.id.clone(),
                bundle: self.dir.path().to_string_lossy().into_owned(),
                stdout: self
                    .dir
                    .path()
                    .join("stdout")
                    .to_string_lossy()
                    .into_owned(),
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
