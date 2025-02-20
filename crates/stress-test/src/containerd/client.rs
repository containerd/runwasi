use std::collections::HashMap;
use std::io::Cursor;
use std::mem::take;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use containerd_client::services::v1::container::Runtime;
use containerd_client::services::v1::containers_client::ContainersClient;
use containerd_client::services::v1::content_client::ContentClient;
use containerd_client::services::v1::images_client::ImagesClient;
use containerd_client::services::v1::leases_client::LeasesClient;
use containerd_client::services::v1::snapshots::PrepareSnapshotRequest;
use containerd_client::services::v1::snapshots::snapshots_client::SnapshotsClient;
use containerd_client::services::v1::tasks_client::TasksClient;
use containerd_client::services::v1::{
    Container as SpecContainer, CreateContainerRequest, CreateRequest, CreateTaskRequest,
    DeleteContainerRequest, DeleteRequest, DeleteTaskRequest, GetImageRequest, KillRequest,
    ReadContentRequest, StartRequest, WaitRequest,
};
use containerd_client::types::Mount;
use humantime::format_rfc3339;
use oci_spec::image::{Arch, ImageConfiguration, ImageIndex, ImageManifest};
use oci_spec::runtime::Spec;
use prost_types::Any;
use tokio_async_drop::tokio_async_drop as async_drop;
use tonic::Request;
use tonic::transport::Channel;

struct ClientInner {
    channel: Channel,
    namespace: String,
    lease: String,
}

pub struct Client(Arc<ClientInner>);

impl ClientInner {
    fn with_metadata<T>(&self, request: T) -> Request<T> {
        let mut request = Request::new(request);
        let metadata = request.metadata_mut();
        metadata.insert("containerd-namespace", self.namespace.parse().unwrap());
        if !self.lease.is_empty() {
            metadata.insert("containerd-lease", self.lease.parse().unwrap());
        }
        request
    }
}

impl ClientInner {
    pub async fn connect(socket: impl AsRef<Path>, namespace: impl Into<String>) -> Result<Self> {
        let channel = containerd_client::connect(socket).await?;
        let namespace = namespace.into();
        let lease = String::new();

        Ok(Self {
            channel,
            namespace,
            lease,
        })
    }

    async fn with_lease(&self) -> Result<Self> {
        let mut client = LeasesClient::new(self.channel.clone());
        let expiry = SystemTime::now() + Duration::from_secs(60 * 60 * 24);

        let request = CreateRequest {
            labels: HashMap::from([(
                "containerd.io/gc.expire".into(),
                format_rfc3339(expiry).to_string(),
            )]),
            ..Default::default()
        };
        let request = self.with_metadata(request);
        let response = client.create(request).await?.into_inner();

        let lease = response.lease.context("creating lease")?;

        Ok(Self {
            channel: self.channel.clone(),
            namespace: self.namespace.clone(),
            lease: lease.id,
        })
    }

    async fn drop_lease(&mut self) -> Result<()> {
        if self.lease.is_empty() {
            return Ok(());
        }

        let mut client = LeasesClient::new(self.channel.clone());

        let id = take(&mut self.lease);
        let request = DeleteRequest { id, sync: false };
        let request = self.with_metadata(request);
        client.delete(request).await?;
        Ok(())
    }

    pub async fn read_content(&self, digest: impl ToString) -> Result<Vec<u8>> {
        let digest = digest.to_string();
        let mut client = ContentClient::new(self.channel.clone());
        let request = ReadContentRequest {
            digest,
            ..Default::default()
        };
        let request = self.with_metadata(request);
        let mut response = client.read(request).await?.into_inner();
        let mut data = vec![];
        while let Some(content) = response.message().await? {
            if content.offset as usize != data.len() {
                bail!("bad offset!");
            }
            data.extend_from_slice(&content.data);
        }

        Ok(data)
    }

    pub async fn image_config(&self, image: String) -> Result<ImageConfiguration> {
        let mut client = ImagesClient::new(self.channel.clone());

        let request = GetImageRequest { name: image };
        let request = self.with_metadata(request);
        let response = client.get(request).await?.into_inner();
        let image = response.image.context("Could not find image")?;
        let descriptor = image.target.context("Could not find image descriptor")?;
        let mut manifest = self.read_content(&descriptor.digest).await?;

        // If this is a multiplatform image, the manifest will be an index manifest
        // rather than an image manifest.
        if let Ok(index) = ImageIndex::from_reader(Cursor::new(&manifest)) {
            let descriptor = index
                .manifests()
                .iter()
                .find(|m| {
                    let Some(platform) = m.platform() else {
                        return false;
                    };
                    match platform.architecture() {
                        Arch::Amd64 => cfg!(target_arch = "x86_64"),
                        Arch::ARM64 => cfg!(target_arch = "aarch64"),
                        _ => false,
                    }
                })
                .context("host platform not supported")?;
            manifest = self.read_content(descriptor.digest()).await?;
        }

        let manifest = ImageManifest::from_reader(Cursor::new(manifest))?;
        let config = self.read_content(manifest.config().digest()).await?;
        let config = ImageConfiguration::from_reader(Cursor::new(config))?;

        Ok(config)
    }

    pub(crate) async fn snapshot(
        &self,
        id: String,
        diffs: Vec<String>,
    ) -> Result<Vec<containerd_client::types::Mount>> {
        let mut client = SnapshotsClient::new(self.channel.clone());
        let request = PrepareSnapshotRequest {
            key: id,
            parent: diffs.join(" "),
            snapshotter: String::from("overlayfs"),
            ..Default::default()
        };
        let request = self.with_metadata(request);
        let response = client.prepare(request).await?.into_inner();
        Ok(response.mounts)
    }

    async fn create_container(
        &self,
        id: impl Into<String>,
        image: impl Into<String>,
        runtime: impl Into<String>,
        spec: Spec,
    ) -> Result<()> {
        let mut client = ContainersClient::new(self.channel.clone());

        let spec = Any {
            type_url: "types.containerd.io/opencontainers/runtime-spec/1/Spec".to_string(),
            value: serde_json::to_vec(&spec).unwrap(),
        };

        let container = SpecContainer {
            id: id.into(),
            image: image.into(),
            runtime: Some(Runtime {
                name: runtime.into(),
                options: None,
            }),
            spec: Some(spec),
            ..Default::default()
        };

        let request = CreateContainerRequest {
            container: Some(container),
        };
        let request = self.with_metadata(request);
        client.create(request).await?;

        Ok(())
    }

    async fn create_task(
        &self,
        container_id: impl Into<String>,
        mounts: impl Into<Vec<Mount>>,
        stdout: impl Into<String>,
    ) -> Result<()> {
        let mut client = TasksClient::new(self.channel.clone());

        let request = CreateTaskRequest {
            container_id: container_id.into(),
            rootfs: mounts.into(),
            stdout: stdout.into(),
            ..Default::default()
        };
        let request = self.with_metadata(request);

        client.create(request).await?;

        Ok(())
    }

    async fn start_task(&self, container_id: impl Into<String>) -> Result<()> {
        let mut client = TasksClient::new(self.channel.clone());

        let request = StartRequest {
            container_id: container_id.into(),
            ..Default::default()
        };
        let request = self.with_metadata(request);

        client.start(request).await?;

        Ok(())
    }

    async fn wait_task(&self, container_id: impl Into<String>) -> Result<()> {
        let mut client = TasksClient::new(self.channel.clone());

        let request = WaitRequest {
            container_id: container_id.into(),
            ..Default::default()
        };
        let request = self.with_metadata(request);

        client.wait(request).await?;

        Ok(())
    }

    async fn kill_task(&self, container_id: impl Into<String>) -> Result<()> {
        let mut client = TasksClient::new(self.channel.clone());

        let request = KillRequest {
            container_id: container_id.into(),
            signal: 9, // SIGKILL
            all: true,
            ..Default::default()
        };
        let request = self.with_metadata(request);

        client.kill(request).await?;

        Ok(())
    }

    async fn delete_task(&self, container_id: impl Into<String>) -> Result<()> {
        let mut client = TasksClient::new(self.channel.clone());

        let request = DeleteTaskRequest {
            container_id: container_id.into(),
        };
        let request = self.with_metadata(request);

        client.delete(request).await?;

        Ok(())
    }

    async fn delete_container(&self, id: impl Into<String>) -> Result<()> {
        let mut client = ContainersClient::new(self.channel.clone());

        let request = DeleteContainerRequest { id: id.into() };
        let request = self.with_metadata(request);

        client.delete(request).await?;

        Ok(())
    }
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        async_drop!({
            let _ = self.drop_lease().await;
        })
    }
}

impl Client {
    pub async fn default() -> Result<Self> {
        Self::connect("/run/containerd/containerd.sock", "default").await
    }

    pub async fn connect(socket: impl AsRef<Path>, namespace: impl Into<String>) -> Result<Self> {
        let inner = ClientInner::connect(socket, namespace)
            .await?
            .with_lease()
            .await?;

        Ok(Self(Arc::new(inner)))
    }

    pub async fn get_mounts(
        &self,
        id: impl Into<String>,
        image: impl Into<String>,
    ) -> Result<Vec<Mount>> {
        let config = self.0.image_config(image.into()).await?;
        let diffs = config.rootfs().diff_ids().clone();
        let mounts = self.0.snapshot(id.into(), diffs).await?;
        Ok(mounts)
    }

    pub async fn entrypoint(&self, image: impl Into<String>) -> Result<Vec<String>> {
        let config = self.0.image_config(image.into()).await?;
        let Some(config) = config.config() else {
            return Ok(vec![]);
        };
        let Some(entrypoint) = config.entrypoint() else {
            return Ok(vec![]);
        };
        Ok(entrypoint.clone())
    }

    pub async fn create_container(
        &self,
        id: impl Into<String>,
        image: impl Into<String>,
        runtime: impl Into<String>,
        spec: Spec,
    ) -> Result<()> {
        self.0.create_container(id, image, runtime, spec).await
    }

    pub async fn create_task(
        &self,
        container_id: impl Into<String>,
        mounts: impl Into<Vec<Mount>>,
        stdout: impl Into<String>,
    ) -> Result<()> {
        self.0.create_task(container_id, mounts, stdout).await
    }

    pub async fn start_task(&self, container_id: impl Into<String>) -> Result<()> {
        self.0.start_task(container_id).await
    }

    pub async fn wait_task(&self, container_id: impl Into<String>) -> Result<()> {
        self.0.wait_task(container_id).await
    }

    pub async fn kill_task(&self, container_id: impl Into<String>) -> Result<()> {
        self.0.kill_task(container_id).await
    }

    pub async fn delete_task(&self, container_id: impl Into<String>) -> Result<()> {
        self.0.delete_task(container_id).await
    }

    pub async fn delete_container(&self, container_id: impl Into<String>) -> Result<()> {
        self.0.delete_container(container_id).await
    }
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
