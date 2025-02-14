use std::path::Path;

use anyhow::Result;

use super::{Client, Shim};

pub struct Containerd {
    containerd: Client,
}

impl Containerd {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            containerd: Client::default().await?,
        })
    }
}

impl crate::traits::Containerd for Containerd {
    type Shim = Shim;
    async fn start_shim(&self, shim: impl AsRef<Path> + Send) -> Result<Shim> {
        Shim::new(self.containerd.clone(), shim).await
    }
}
