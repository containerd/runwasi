use std::path::Path;

use anyhow::Result;

#[trait_variant::make(Send)]
pub trait Containerd {
    type Shim: Shim;
    async fn start_shim(&self, shim: impl AsRef<Path> + Send) -> Result<Self::Shim>;
}

#[trait_variant::make(Send)]
pub trait Shim {
    type Task: Task;
    async fn task<T: Into<String>>(
        &self,
        image: impl Into<String> + Send,
        args: impl IntoIterator<Item = T> + Send,
    ) -> Result<Self::Task>;
}

#[trait_variant::make(Send)]
pub trait Task {
    async fn create(&self, verbose: bool) -> Result<()>;
    async fn start(&self) -> Result<()>;
    async fn wait(&self) -> Result<()>;
    async fn delete(&self) -> Result<()>;
}
