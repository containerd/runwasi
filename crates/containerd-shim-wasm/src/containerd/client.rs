#![cfg(unix)]

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::{DefaultHasher, Hash, Hasher as _};
use std::path::Path;

use containerd_client::services::v1::containers_client::ContainersClient;
use containerd_client::services::v1::content_client::ContentClient;
use containerd_client::services::v1::images_client::ImagesClient;
use containerd_client::services::v1::leases_client::LeasesClient;
use containerd_client::services::v1::{
    Container, DeleteContentRequest, GetContainerRequest, GetImageRequest, Image, Info,
    InfoRequest, ReadContentRequest, UpdateRequest, WriteAction, WriteContentRequest,
    WriteContentResponse,
};
use containerd_client::tonic::Streaming;
use containerd_client::tonic::transport::Channel;
use containerd_client::{tonic, with_namespace};
use containerd_shimkit::sandbox::error::{Error as ShimError, Result};
use futures::TryStreamExt;
use oci_spec::image::{Arch, Digest, ImageManifest, MediaType, Platform};
use sha256::digest;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request};

use super::lease::LeaseGuard;
use crate::sandbox::context::WasmLayer;
use crate::shim::Compiler;

// Adds lease info to grpc header
// https://github.com/containerd/containerd/blob/8459273f806e068e1a6bacfaf1355bbbad738d5e/docs/garbage-collection.md#using-grpc
macro_rules! with_lease {
    ($req : ident, $ns: expr, $lease_id: expr) => {{
        let mut req = Request::new($req);
        let md = req.metadata_mut();
        // https://github.com/containerd/containerd/blob/main/namespaces/grpc.go#L27
        md.insert("containerd-namespace", $ns.parse().unwrap());
        md.insert("containerd-lease", $lease_id.parse().unwrap());
        req
    }};
}

static PRECOMPILE_PREFIX: &str = "runwasi.io/precompiled";
// 16MB is the default maximum gRPC message size for gRPC in containerd:
// https://github.com/containerd/containerd/blob/main/defaults/defaults.go
// Conservatively set the max to 15MB to leave room for message overhead
static MAX_WRITE_CHUNK_SIZE_BYTES: i64 = 1024 * 1024 * 15;

#[derive(Debug)]
pub struct Client {
    inner: Channel,
    namespace: String,
}

#[derive(Debug)]
pub(crate) struct WriteContent {
    lease: LeaseGuard,
    pub digest: String,
}

impl WriteContent {
    // used in tests
    #[allow(dead_code)]
    pub async fn release(self) -> anyhow::Result<()> {
        self.lease.release().await
    }
}

// sync wrapper implementation from https://tokio.rs/tokio/topics/bridging
impl Client {
    // wrapper around connection that will establish a connection and create a client
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    pub async fn connect(
        address: impl AsRef<Path> + std::fmt::Debug,
        namespace: impl Into<String> + std::fmt::Debug,
    ) -> Result<Client> {
        let inner = containerd_client::connect(address.as_ref())
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?;

        Ok(Client {
            inner,
            namespace: namespace.into(),
        })
    }

    // wrapper around read that will read the entire content file
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn read_content(&self, digest: impl ToString + std::fmt::Debug) -> Result<Vec<u8>> {
        let req = ReadContentRequest {
            digest: digest.to_string(),
            ..Default::default()
        };
        let req = with_namespace!(req, self.namespace);
        ContentClient::new(self.inner.clone())
            .read(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?
            .into_inner()
            .map_ok(|msg| msg.data)
            .try_concat()
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))
    }

    // used in tests to clean up content
    #[allow(dead_code)]
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn delete_content(&self, digest: impl ToString + std::fmt::Debug) -> Result<()> {
        let req = DeleteContentRequest {
            digest: digest.to_string(),
        };
        let req = with_namespace!(req, self.namespace);
        ContentClient::new(self.inner.clone())
            .delete(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?;
        Ok(())
    }

    // wrapper around lease that will create a lease and return a guard that will delete the lease when dropped
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn lease(&self, reference: String) -> Result<LeaseGuard> {
        let mut lease_labels = HashMap::new();
        // Unwrap is safe here since 24 hours is a valid time
        let expire = chrono::Utc::now() + chrono::Duration::try_hours(24).unwrap();
        lease_labels.insert("containerd.io/gc.expire".to_string(), expire.to_rfc3339());
        let lease_request = containerd_client::services::v1::CreateRequest {
            id: reference.clone(),
            labels: lease_labels,
        };

        let mut leases_client = LeasesClient::new(self.inner.clone());
        let lease = leases_client
            .create(with_namespace!(lease_request, self.namespace))
            .await
            .map_err(|e| ShimError::Containerd(e.to_string()))?
            .into_inner()
            .lease
            .ok_or_else(|| {
                ShimError::Containerd(format!("unable to create lease for  {}", reference))
            })?;

        Ok(LeaseGuard::new(
            leases_client.clone(),
            lease.id,
            self.namespace.clone(),
        ))
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn save_content(
        &self,
        data: Vec<u8>,
        unique_id: &str,
        labels: HashMap<String, String>,
    ) -> Result<WriteContent> {
        let expected = format!("sha256:{}", digest(data.clone()));
        let reference = format!("precompile-{}", unique_id);
        let lease = self.lease(reference.clone()).await?;

        let digest = 'digest: {
            // create a channel to feed the stream; only sending one message at a time so we can set this to one
            let (tx, rx) = mpsc::channel(1);

            let len = data.len() as i64;
            log::debug!("Writing {} bytes to content store", len);
            let mut client = ContentClient::new(self.inner.clone());

            // Send write request with Stat action to containerd to let it know that we are going to write content
            // if the content is already there, it will return early with AlreadyExists
            let req = WriteContentRequest {
                r#ref: reference.clone(),
                action: WriteAction::Stat.into(),
                expected: expected.clone(),
                ..Default::default()
            };
            tx.send(req)
                .await
                .map_err(|err| ShimError::Containerd(err.to_string()))?;

            // Create stream for the channel
            let request_stream = ReceiverStream::new(rx);
            let request_stream = with_lease!(request_stream, self.namespace, lease.id());
            let mut response_stream = match client.write(request_stream).await {
                Ok(response_stream) => response_stream.into_inner(),
                Err(e) if e.code() == Code::AlreadyExists => {
                    log::info!("content already exists {expected}");
                    break 'digest expected;
                }
                Err(e) => return Err(ShimError::Containerd(e.to_string())),
            };

            // Get initial Stat response
            let response = response_stream
                .message()
                .await
                .map_err(|e| ShimError::Containerd(e.to_string()))?
                .ok_or_else(|| {
                    ShimError::Containerd(format!(
                        "no response received after write request for {}",
                        expected
                    ))
                })?;
            log::debug!(
                "Starting to write content for layer {} with current status response {:?}",
                expected,
                response
            );

            // Separate the content into chunks and send a write request for each chunk.
            let mut offset = response.offset;
            while offset < len {
                let end = (offset + MAX_WRITE_CHUNK_SIZE_BYTES).min(len);
                let chunk = &data[offset as usize..end as usize];

                let write_request = WriteContentRequest {
                    action: WriteAction::Write.into(),
                    // Ignore size verification of each chunk
                    total: 0,
                    offset,
                    data: chunk.to_vec(),
                    ..Default::default()
                };
                let response =
                    send_message(write_request, &mut response_stream, &tx, &expected).await?;
                log::debug!(
                    "Writing content for layer {} at offset {} got response: {:?}",
                    expected,
                    offset,
                    response
                );
                offset = end;
            }

            // Send a final empty commit request to end the transaction
            let commit_request = WriteContentRequest {
                action: WriteAction::Commit.into(),
                total: len,
                offset: len,
                expected: expected.clone(),
                labels,
                data: Vec::new(),
                ..Default::default()
            };
            let response =
                send_message(commit_request, &mut response_stream, &tx, &expected).await?;
            log::info!(
                "Validating final response after writing content for layer {}: {:?}",
                expected,
                response
            );

            // Client should validate that all bytes were written and that the digest matches
            if response.offset != len {
                return Err(ShimError::Containerd(format!(
                    "failed to write all bytes, expected {} got {}",
                    len, response.offset
                )));
            }
            if response.digest != expected {
                return Err(ShimError::Containerd(format!(
                    "unexpected digest, expected {} got {}",
                    expected, response.digest
                )));
            }
            response.digest
        };

        Ok(WriteContent { lease, digest })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn get_info(&self, content_digest: &Digest) -> Result<Info> {
        let req = InfoRequest {
            digest: content_digest.to_string(),
        };
        let req = with_namespace!(req, self.namespace);
        let info = ContentClient::new(self.inner.clone())
            .info(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?
            .into_inner()
            .info
            .ok_or_else(|| {
                ShimError::Containerd(format!("failed to get info for content {}", content_digest))
            })?;
        Ok(info)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn update_info(&self, info: Info) -> Result<Info> {
        let mut req = UpdateRequest {
            info: Some(info.clone()),
            update_mask: Some(Default::default()),
        };
        // Instantiate update_mask to Default and then mutate it to avoid namig it's type.
        // The type is `prost_types::FieldMask` and not re-exported, naming it would require depending on it.
        // Depending on it would mean keeping it's version in sync with the version in `containerd-client`.
        req.update_mask.as_mut().unwrap().paths = vec!["labels".to_string()];
        let req = with_namespace!(req, self.namespace);
        let info = ContentClient::new(self.inner.clone())
            .update(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?
            .into_inner()
            .info
            .ok_or_else(|| {
                ShimError::Containerd(format!("failed to update info for content {}", info.digest))
            })?;
        Ok(info)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn get_image(&self, image_name: impl ToString + std::fmt::Debug) -> Result<Image> {
        let name = image_name.to_string();
        let req = GetImageRequest { name };
        let req = with_namespace!(req, self.namespace);
        let image = ImagesClient::new(self.inner.clone())
            .get(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?
            .into_inner()
            .image
            .ok_or_else(|| {
                ShimError::Containerd(format!(
                    "failed to get image for image {}",
                    image_name.to_string()
                ))
            })?;
        Ok(image)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    fn extract_image_content_sha(&self, image: &Image) -> Result<String> {
        let digest = image
            .target
            .as_ref()
            .ok_or_else(|| {
                ShimError::Containerd(format!(
                    "failed to get image content sha for image {}",
                    image.name
                ))
            })?
            .digest
            .clone();
        Ok(digest)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn get_container(&self, container_name: impl AsRef<str> + Debug) -> Result<Container> {
        let container_name = container_name.as_ref();
        let id = container_name.to_string();
        let req = GetContainerRequest { id };
        let req = with_namespace!(req, self.namespace);
        let container = ContainersClient::new(self.inner.clone())
            .get(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?
            .into_inner()
            .container
            .ok_or_else(|| {
                ShimError::Containerd(format!(
                    "failed to get image for container {container_name}",
                ))
            })?;
        Ok(container)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn get_image_manifest_and_digest(
        &self,
        image_name: &str,
    ) -> Result<(ImageManifest, Digest)> {
        let image = self.get_image(image_name).await?;
        let image_digest = self.extract_image_content_sha(&image)?.try_into()?;
        let manifest =
            ImageManifest::from_reader(self.read_content(&image_digest).await?.as_slice())?;
        Ok((manifest, image_digest))
    }

    // load module will query the containerd store to find an image that has an OS of type 'wasm'
    // If found it continues to parse the manifest and return the layers that contains the WASM modules
    // and possibly other configuration layers.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(compiler), level = "Debug")
    )]
    pub async fn load_modules(
        &self,
        containerd_id: impl AsRef<str> + Debug,
        engine_name: impl AsRef<str> + Debug,
        supported_layer_types: &[&str],
        compiler: Option<&impl Compiler>,
    ) -> Result<(Vec<WasmLayer>, Platform)> {
        let container = self.get_container(containerd_id).await?;
        let (manifest, image_digest) = self.get_image_manifest_and_digest(&container.image).await?;

        let image_config_descriptor = manifest.config();
        let image_config = self.read_content(image_config_descriptor.digest()).await?;
        let image_config = image_config.as_slice();

        // the only part we care about here is the platform values
        let platform: Platform = serde_json::from_slice(image_config)?;
        let Arch::Wasm = platform.architecture() else {
            log::info!("manifest is not in WASM OCI image format");
            return Ok((vec![], platform));
        };

        log::info!("found manifest with WASM OCI image format");
        // This label is unique across runtimes and version of the shim running
        // a precompiled component/module will not work across different runtimes or versions

        let configs = manifest
            .layers()
            .iter()
            .filter(|x| is_wasm_layer(x.media_type(), supported_layer_types))
            .collect::<Vec<_>>();

        if configs.is_empty() {
            log::info!("no WASM layers found in OCI image");
            return Ok((vec![], platform));
        }

        log::info!("using OCI layers");

        let Some(compiler) = compiler else {
            let mut layers = vec![];
            for config in configs {
                let layer = self.read_original_layer(config).await?;
                layers.push(layer);
            }
            return Ok((layers, platform));
        };

        let precompile_id = precompile_label(engine_name.as_ref(), compiler.cache_key());

        let image_info = self.get_info(&image_digest).await?;
        let mut needs_precompile = !image_info.labels.contains_key(&precompile_id);

        let mut layers = vec![];
        for original_config in configs {
            let layer = match self
                .read_precompiled_layer(original_config, &precompile_id)
                .await
            {
                Ok(layer) => layer,
                Err(err) => {
                    log::error!("failed to load precompiled layer: {err}");
                    log::error!("falling back to original layer and marking for recompile");
                    needs_precompile = true;
                    self.read_original_layer(original_config).await?
                }
            };
            layers.push(layer);
        }

        if needs_precompile {
            log::info!("precompiling layers for image: {}", container.image);
            let compiled_layers = match compiler.compile(&layers).await {
                Ok(compiled_layers) => {
                    if compiled_layers.len() != layers.len() {
                        return Err(ShimError::FailedPrecondition(
                            "precompile returned wrong number of layers".to_string(),
                        ));
                    }
                    compiled_layers
                }
                Err(e) => {
                    log::error!("precompilation failed: {}", e);
                    return Ok((layers, platform));
                }
            };

            let mut layers_for_runtime = Vec::with_capacity(compiled_layers.len());
            for (i, compiled_layer) in compiled_layers.iter().enumerate() {
                if compiled_layer.is_none() {
                    log::debug!("no compiled layer using original");
                    layers_for_runtime.push(layers[i].clone());
                    continue;
                }

                let compiled_layer = compiled_layer.as_ref().unwrap();
                let original_config = &layers[i].config;
                let labels = HashMap::from([(
                    format!("{precompile_id}/original"),
                    original_config.digest().to_string(),
                )]);
                let precompiled_content = self
                    .save_content(compiled_layer.clone(), &precompile_id, labels)
                    .await?;

                log::debug!(
                    "updating original layer {} with compiled layer {}",
                    original_config.digest(),
                    precompiled_content.digest
                );
                // We add two labels here:
                // - one with cache key per engine instance
                // - one with a gc ref flag so it doesn't get cleaned up as long as the original layer exists
                let mut original_layer = self.get_info(original_config.digest()).await?;
                original_layer
                    .labels
                    .insert(precompile_id.clone(), precompiled_content.digest.clone());
                original_layer.labels.insert(
                    format!("containerd.io/gc.ref.content.precompile.{}", i),
                    precompiled_content.digest.clone(),
                );
                self.update_info(original_layer).await?;

                // The original image is considered a root object, by adding a ref to the new compiled content
                // We tell containerd to not garbage collect the new content until this image is removed from the system
                // this ensures that we keep the content around after the lease is dropped
                // We also save the precompiled flag here since the image labels can be mutated containerd, for example if the image is pulled twice
                log::debug!(
                    "updating image content with precompile digest to avoid garbage collection"
                );
                let mut image_content = self.get_info(&image_digest).await?;
                image_content.labels.insert(
                    format!("containerd.io/gc.ref.content.precompile.{}", i),
                    precompiled_content.digest,
                );
                image_content
                    .labels
                    .insert(precompile_id.clone(), "true".to_string());
                self.update_info(image_content).await?;

                layers_for_runtime.push(WasmLayer {
                    config: original_config.clone(),
                    layer: compiled_layer.clone(),
                });

                let _ = precompiled_content.lease.release().await;
            }
            return Ok((layers_for_runtime, platform));
        };

        log::info!("using OCI layers");
        Ok((layers, platform))
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn read_precompiled_layer(
        &self,
        config: &oci_spec::image::Descriptor,
        precompile_id: &String,
    ) -> Result<WasmLayer, ShimError> {
        let digest = config.digest().clone();
        let info = self.get_info(&digest).await?;
        let Some(label) = info.labels.get(precompile_id) else {
            return Err(ShimError::NotFound(String::from(
                "precompiled layer not found",
            )));
        };
        let digest: Digest = label.parse()?;
        log::info!(
            "layer {} has pre-compiled content: {} ",
            info.digest,
            &digest
        );
        self.read_content(digest).await.map(|module| WasmLayer {
            config: config.clone(),
            layer: module,
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    async fn read_original_layer(
        &self,
        config: &oci_spec::image::Descriptor,
    ) -> Result<WasmLayer, ShimError> {
        let digest = config.digest();
        log::debug!("loading digest: {} ", digest);
        self.read_content(digest).await.map(|module| WasmLayer {
            config: config.clone(),
            layer: module,
        })
    }
}

fn precompile_label(name: &str, version: impl Hash) -> String {
    let version = {
        let mut hasher = DefaultHasher::new();
        version.hash(&mut hasher);
        hasher.finish().to_string()
    };
    format!("{}/{}/{}", PRECOMPILE_PREFIX, name, version)
}

fn is_wasm_layer(media_type: &MediaType, supported_layer_types: &[&str]) -> bool {
    let supported = supported_layer_types.contains(&media_type.to_string().as_str());
    log::debug!(
        "layer type {} is supported: {}",
        media_type.to_string().as_str(),
        supported
    );
    supported
}

async fn send_message(
    request: WriteContentRequest,
    response_stream: &mut Streaming<WriteContentResponse>,
    tx: &mpsc::Sender<WriteContentRequest>,
    digest: &str,
) -> Result<WriteContentResponse> {
    tx.send(request)
        .await
        .map_err(|err| ShimError::Containerd(format!("commit request error: {}", err)))?;
    response_stream
        .message()
        .await
        .map_err(|err| ShimError::Containerd(format!("response stream error: {}", err)))?
        .ok_or_else(|| {
            ShimError::Containerd(format!(
                "no response received after write content request for {}",
                digest
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::hash::Hash;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI32, Ordering};

    use oci_tar_builder::WASM_LAYER_MEDIA_TYPE;

    use super::*;
    use crate::shim::NO_COMPILER;
    use crate::testing::oci_helpers::ImageContent;
    use crate::testing::{TEST_NAMESPACE, oci_helpers};

    #[tokio::test(flavor = "current_thread")]
    async fn test_save_content() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, "test-ns").await.unwrap();
        let data = b"hello world".to_vec();

        let expected = digest(data.clone());
        let expected = format!("sha256:{}", expected);

        let label = HashMap::from([(precompile_label("test", "hasdfh"), "original".to_string())]);
        let returned = client
            .save_content(data, "test", label.clone())
            .await
            .unwrap();
        assert_eq!(expected, returned.digest.clone());

        let data = client.read_content(returned.digest.clone()).await.unwrap();
        assert_eq!(data, b"hello world");

        client
            .save_content(data.clone(), "test", label.clone())
            .await
            .expect_err("Should not be able to save when lease is open");

        // need to drop the lease to be able to create a second one
        // a second call should be successful since it already exists
        let _ = returned.release().await;

        // a second call should be successful since it already exists
        let returned = client
            .save_content(data, "test", label.clone())
            .await
            .unwrap();
        assert_eq!(expected, returned.digest);

        client.delete_content(expected.clone()).await.unwrap();

        client
            .read_content(expected)
            .await
            .expect_err("content should not exist");

        let _ = returned.release().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_when_precompile_not_supported() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).await.unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let (layers, _) = client
            .load_modules(
                container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                NO_COMPILER.as_ref(),
            )
            .await
            .unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_bytes.bytes);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_precompiled_once() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE)
            .await
            .unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecomipler::new();
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);

        let (_, _) = client
            .load_modules(
                &container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);

        // Even on second calls should only pre-compile once
        let (layers, _) = client
            .load_modules(
                &container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_recompiled_if_version_changes() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE)
            .await
            .unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecomipler::new();
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);

        let (_, _) = client
            .load_modules(
                &container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);

        engine.precompile_id = "new_version".to_string();
        let (_, _) = client
            .load_modules(
                &container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_precompiled() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE)
            .await
            .unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecomipler::new();
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);
        let expected_id = precompile_label("fake", &engine.cache_key());

        let (layers, _) = client
            .load_modules(
                container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);

        let (manifest, _) = client
            .get_image_manifest_and_digest(&image_name)
            .await
            .unwrap();
        let original_config = manifest.layers().first().unwrap();
        let info = client.get_info(original_config.digest()).await.unwrap();

        let actual_digest = info.labels.get(&expected_id).unwrap();
        assert_eq!(
            actual_digest.to_string(),
            format!("sha256:{}", &digest(fake_precompiled_bytes.bytes.clone()))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_precompiled_but_not_for_all_layers() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE)
            .await
            .unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let non_wasm_bytes = generate_content("original_dont_compile", "textfile");
        let (_image_name, container_name, _cleanup) =
            generate_test_container(None, &[&fake_bytes, &non_wasm_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecomipler::new();
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);

        let (layers, _) = client
            .load_modules(
                container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE, "textfile"],
                Some(&engine),
            )
            .await
            .unwrap();

        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(engine.layers_compiled_per_call.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
        assert_eq!(layers[1].layer, non_wasm_bytes.bytes);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_do_not_need_precompiled_if_new_layers_are_added_to_existing_image() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE)
            .await
            .unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecomipler::new();
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);

        let (layers, _) = client
            .load_modules(
                container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);

        // get original image sha before importing new image
        let image_sha = client
            .get_image(&_image_name)
            .await
            .unwrap()
            .target
            .unwrap()
            .digest;

        let fake_bytes2 = generate_content("image2", WASM_LAYER_MEDIA_TYPE);
        let (_image_name2, container_name2, _cleanup2) =
            generate_test_container(Some(_image_name), &[&fake_bytes, &fake_bytes2]);
        let fake_precompiled_bytes2 = generate_content("precompiled2", WASM_LAYER_MEDIA_TYPE);
        engine.add_precompiled_bits(fake_bytes2.bytes.clone(), &fake_precompiled_bytes2);

        // When a new image with the same name is create the older image content will disappear
        // but since these layers are part of the new image we don't want to have to recompile
        // for the test, let the original image get removed (which would remove any associated content)
        // and then check that the layers don't need to be recompiled
        oci_helpers::wait_for_content_removal(&image_sha).unwrap();

        let (layers, _) = client
            .load_modules(
                container_name2,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 2);
        assert_eq!(layers.len(), 2);
        assert_eq!(engine.layers_compiled_per_call.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_do_not_need_precompiled_if_new_layers_are_add_to_new_image() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE)
            .await
            .unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecomipler::new();
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);

        let (layers, _) = client
            .load_modules(
                container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);

        let fake_bytes2 = generate_content("image2", WASM_LAYER_MEDIA_TYPE);
        let (_image_name2, container_name2, _cleanup2) =
            generate_test_container(None, &[&fake_bytes, &fake_bytes2]);
        let fake_precompiled_bytes2 = generate_content("precompiled2", WASM_LAYER_MEDIA_TYPE);
        engine.add_precompiled_bits(fake_bytes2.bytes.clone(), &fake_precompiled_bytes2);

        let (layers, _) = client
            .load_modules(
                container_name2,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 2);
        assert_eq!(layers.len(), 2);
        assert_eq!(engine.layers_compiled_per_call.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_precompiled_for_multiple_layers() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE)
            .await
            .unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let fake_bytes2 = generate_content("original1", WASM_LAYER_MEDIA_TYPE);

        let (image_name, container_name, _cleanup) =
            generate_test_container(None, &[&fake_bytes, &fake_bytes2]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let fake_precompiled_bytes2 = generate_content("precompiled1", WASM_LAYER_MEDIA_TYPE);

        let mut engine = FakePrecomipler::new();
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);
        engine.add_precompiled_bits(fake_bytes2.bytes.clone(), &fake_precompiled_bytes2);

        let expected_id = precompile_label("fake", &engine.cache_key());

        let (layers, _) = client
            .load_modules(
                container_name,
                "fake",
                &[WASM_LAYER_MEDIA_TYPE],
                Some(&engine),
            )
            .await
            .unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(engine.layers_compiled_per_call.load(Ordering::SeqCst), 2);

        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
        assert_eq!(layers[1].layer, fake_precompiled_bytes2.bytes);

        let (manifest, _) = client
            .get_image_manifest_and_digest(&image_name)
            .await
            .unwrap();

        let original_config1 = manifest.layers().first().unwrap();
        let info1 = client.get_info(original_config1.digest()).await.unwrap();
        let actual_digest1 = info1.labels.get(&expected_id).unwrap();
        assert_eq!(
            actual_digest1.to_string(),
            format!("sha256:{}", &digest(fake_precompiled_bytes.bytes.clone()))
        );

        let original_config2 = manifest.layers().last().unwrap();
        let info2 = client.get_info(original_config2.digest()).await.unwrap();
        let actual_digest2 = info2.labels.get(&expected_id).unwrap();
        assert_eq!(
            actual_digest2.to_string(),
            format!("sha256:{}", &digest(fake_precompiled_bytes2.bytes.clone()))
        );
    }

    fn generate_test_container(
        name: Option<String>,
        original: &[&oci_helpers::ImageContent],
    ) -> (String, String, oci_helpers::OCICleanup) {
        let _ = env_logger::try_init();

        let random_number = random_number();
        let image_name = name.unwrap_or(format!("localhost/test:latest{}", random_number));
        oci_helpers::import_image(&image_name, original).unwrap();

        let container_name = format!("test-container-{}", random_number);
        oci_helpers::create_container(&container_name, &image_name).unwrap();

        let _cleanup = oci_helpers::OCICleanup {
            image_name: image_name.clone(),
            container_name: container_name.clone(),
        };
        (image_name, container_name, _cleanup)
    }

    fn generate_content(seed: &str, media_type: &str) -> oci_helpers::ImageContent {
        let mut content = seed.as_bytes().to_vec();
        for _ in 0..100 {
            content.push(random_number() as u8);
        }
        ImageContent {
            bytes: content,
            media_type: media_type.to_string(),
        }
    }

    fn random_number() -> u32 {
        let x: u32 = rand::random();
        x
    }

    #[derive(Clone, Debug)]
    struct FakePrecomipler {
        precompile_id: String,
        precompiled_layers: HashMap<String, Vec<u8>>,
        precompile_called: Arc<AtomicI32>,
        layers_compiled_per_call: Arc<AtomicI32>,
    }

    impl FakePrecomipler {
        fn new() -> Self {
            let precompile_id = format!("uuid-{}", random_number());
            FakePrecomipler {
                precompile_id,
                precompiled_layers: HashMap::new(),
                precompile_called: Arc::new(AtomicI32::new(0)),
                layers_compiled_per_call: Arc::new(AtomicI32::new(0)),
            }
        }
        fn add_precompiled_bits(
            &mut self,
            original: Vec<u8>,
            precompiled_content: &oci_helpers::ImageContent,
        ) {
            let key = digest(original);
            self.precompiled_layers
                .insert(key, precompiled_content.bytes.clone());
        }
    }

    impl Compiler for FakePrecomipler {
        fn cache_key(&self) -> impl Hash {
            self.precompile_id.clone()
        }

        async fn compile(&self, layers: &[WasmLayer]) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
            self.layers_compiled_per_call.store(0, Ordering::SeqCst);
            self.precompile_called.fetch_add(1, Ordering::SeqCst);
            let mut compiled_layers = vec![];
            for layer in layers {
                if layer.config.media_type().to_string() == *"textfile" {
                    // simulate a layer that can't be precompiled
                    compiled_layers.push(None);
                    continue;
                }

                let key = digest(layer.layer.clone());
                if self.precompiled_layers.values().any(|l| digest(l) == key) {
                    // simulate scenario were one of the layers is already compiled
                    compiled_layers.push(None);
                    continue;
                }

                // load the "precompiled" layer that was stored as precompiled for this layer
                self.precompiled_layers.iter().all(|x| {
                    log::warn!("layer: {:?}", x.0);
                    true
                });
                let precompiled = self.precompiled_layers[&key].clone();
                compiled_layers.push(Some(precompiled));
                self.layers_compiled_per_call.fetch_add(1, Ordering::SeqCst);
            }
            Ok(compiled_layers)
        }
    }
}
