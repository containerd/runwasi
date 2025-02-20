use anyhow::{Result, bail};
use trapeze::{Client, Code};

use crate::protos::containerd::task::v2::*;
use crate::protos::containerd::task::v3::Task as TaskV3;

#[derive(Clone, Copy)]
enum Version {
    V2,
    V3,
}

#[derive(Clone)]
pub struct TaskClient {
    client: Client,
    version: Version,
}

macro_rules! multiplex {
    ($obj:ident.$method:ident ( $req:ident ) $($rest:tt)*) => {{
        match $obj.version {
            Version::V2 => {
                trapeze::as_client!(&$obj.client: Task)
                    .$method($req)
                    .await
            }
            Version::V3 => {
                trapeze::as_client!(&$obj.client: TaskV3)
                    .$method($req)
                    .await
            }
        }
    }};
}

impl TaskClient {
    pub async fn connect(address: impl AsRef<str>) -> Result<Self> {
        let client = Client::connect(address).await?;

        let version = 'v: {
            let task = trapeze::as_client!(&client: Task);
            let Err(status) = task.delete(DeleteRequest::default()).await else {
                bail!("unexpected shim response")
            };
            if status.code() != Code::Unimplemented {
                break 'v Version::V2;
            }
            let task = trapeze::as_client!(&client: TaskV3);
            let Err(status) = task.delete(DeleteRequest::default()).await else {
                bail!("unexpected shim response")
            };
            if status.code() != Code::Unimplemented {
                break 'v Version::V3;
            }
            bail!("unknown task service version")
        };

        Ok(Self { version, client })
    }

    pub async fn shutdown(&self, req: ShutdownRequest) -> trapeze::Result<()> {
        multiplex!(self.shutdown(req))
    }

    pub async fn create(&self, req: CreateTaskRequest) -> trapeze::Result<CreateTaskResponse> {
        multiplex!(self.create(req))
    }

    pub async fn start(&self, req: StartRequest) -> trapeze::Result<StartResponse> {
        multiplex!(self.start(req))
    }

    pub async fn wait(&self, req: WaitRequest) -> trapeze::Result<WaitResponse> {
        multiplex!(self.wait(req))
    }

    pub async fn delete(&self, req: DeleteRequest) -> trapeze::Result<DeleteResponse> {
        multiplex!(self.delete(req))
    }
}
