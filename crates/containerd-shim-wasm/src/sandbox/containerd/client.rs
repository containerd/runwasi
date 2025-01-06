#![cfg(unix)]

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use containerd_client;
use containerd_client::services::v1::containers_client::ContainersClient;
use containerd_client::services::v1::content_client::ContentClient;
use containerd_client::services::v1::images_client::ImagesClient;
use containerd_client::services::v1::leases_client::LeasesClient;
use containerd_client::services::v1::{
    Container, DeleteContentRequest, GetContainerRequest, GetImageRequest, Image, Info,
    InfoRequest, ReadContentRequest, UpdateRequest, WriteAction, WriteContentRequest,
    WriteContentResponse,
};
use containerd_client::tonic::transport::Channel;
use containerd_client::tonic::Streaming;
use containerd_client::{tonic, with_namespace};
use futures::TryStreamExt;
use oci_spec::image::{Arch, DescriptorBuilder, Digest, ImageManifest, MediaType, Platform};
use sha256::digest;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request};

use super::lease::LeaseGuard;
use crate::container::{Engine, PrecompiledLayer};
use crate::sandbox::error::{Error as ShimError, Result};
use crate::sandbox::oci::{self, WasmLayer};
use crate::with_lease;

static PRECOMPILE_PREFIX: &str = "runwasi.io/precompiled";
// 16MB is the default maximum gRPC message size for gRPC in containerd:
// https://github.com/containerd/containerd/blob/main/defaults/defaults.go
// Conservatively set the max to 15MB to leave room for message overhead
static MAX_WRITE_CHUNK_SIZE_BYTES: i64 = 1024 * 1024 * 15;

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
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    pub async fn connect(
        address: impl AsRef<Path>,
        namespace: impl Into<String>,
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
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn read_content(&self, digest: impl ToString) -> Result<Vec<u8>> {
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
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn delete_content(&self, digest: impl ToString) -> Result<()> {
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
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
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
                    log::info!("content already exists {}", expected.clone().to_string());
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

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn get_image(&self, image_name: impl ToString) -> Result<Image> {
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

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn get_container(&self, container_name: impl ToString) -> Result<Container> {
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
                    "failed to get image for container {}",
                    container_name.to_string()
                ))
            })?;
        Ok(container)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
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
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    pub async fn load_modules<T: Engine>(
        &self,
        containerd_id: impl ToString,
        engine: &T,
    ) -> Result<(Vec<oci::WasmLayer>, Platform)> {
        let container = self.get_container(containerd_id.to_string()).await?;
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
        let (can_precompile, precompile_id) = match engine.can_precompile() {
            Some(precompile_id) => (true, precompile_label(T::name(), &precompile_id)),
            None => (false, "".to_string()),
        };

        let image_info = self.get_info(&image_digest).await?;
        let mut needs_precompile =
            can_precompile && !image_info.labels.contains_key(&precompile_id);
        let configs = manifest
            .layers()
            .iter()
            .filter(|x| is_wasm_layer(x.media_type(), T::supported_layers_types()));

        let mut all_layers = HashMap::new();
        let media_type_label = precompile_label(T::name(), "media-type");
        for original_config in configs {
            self.read_wasm_layer(
                original_config,
                can_precompile,
                &precompile_id,
                &mut needs_precompile,
                &media_type_label,
                &mut all_layers,
            )
            .await?;
        }

        let layers = all_layers.into_values().collect::<Vec<_>>();

        if layers.is_empty() {
            log::info!("no WASM layers found in OCI image");
            return Ok((vec![], platform));
        }

        if needs_precompile {
            log::info!("precompiling layers for image: {}", container.image);
            let compiled_layers = match engine.precompile(&layers).await {
                Ok(compiled_layers) => {
                    if compiled_layers.is_empty() {
                        log::info!("no precompiled layers returned");
                        return Ok((layers, platform));
                    }
                    compiled_layers
                }
                Err(e) => {
                    log::error!("precompilation failed: {}", e);
                    return Ok((layers, platform));
                }
            };

            let mut layers_for_runtime = Vec::with_capacity(compiled_layers.len());
            for compiled_layer in compiled_layers.iter() {
                let PrecompiledLayer {
                    media_type,
                    bytes,
                    parents,
                } = compiled_layer;

                if parents.is_empty() {
                    return Err(ShimError::FailedPrecondition(
                        "precompile returned new layer with empty parents".to_string(),
                    ));
                }

                let mut labels = HashMap::new();
                let media_type_label = precompile_label(T::name(), "media-type");
                labels.insert(media_type_label, media_type.clone());

                let precompiled_content = self
                    .save_content(bytes.clone(), &precompile_id, labels)
                    .await?;

                // Update the original layers with a gc label which associates the original digests that
                // were used to process and produce the new layer with the digest of the precompiled content.
                // TODO: parallelize this
                for parent_digest_str in parents {
                    let parent_digest = Digest::from_str(parent_digest_str)?;

                    let mut parent_layer = self.get_info(&parent_digest).await?;

                    let child_digest = precompiled_content.digest.clone();

                    log::debug!(
                        "updating original layer {} with compiled layer {}",
                        parent_digest,
                        child_digest,
                    );

                    let parent_label = format!("{precompile_id}/child.{child_digest}");
                    parent_layer
                        .labels
                        .insert(parent_label, child_digest.clone());

                    let gc_label =
                        format!("containerd.io/gc.ref.content.precompile.{child_digest}");
                    parent_layer.labels.insert(gc_label, child_digest.clone());

                    self.update_info(parent_layer).await?;
                }

                // The original image is considered a root object, by adding a ref to the new compiled content
                // We tell containerd to not garbage collect the new content until this image is removed from the system
                // this ensures that we keep the content around after the lease is dropped
                // We also save the precompiled flag here since the image labels can be mutated containerd, for example if the image is pulled twice
                log::debug!(
                    "updating image content with precompile digest to avoid garbage collection"
                );
                let mut image_content = self.get_info(&image_digest).await?;

                image_content.labels.insert(
                    format!(
                        "containerd.io/gc.ref.content.precompile.{}",
                        precompiled_content.digest
                    ),
                    precompiled_content.digest.clone(),
                );
                image_content
                    .labels
                    .insert(precompile_id.clone(), "true".to_string());
                self.update_info(image_content).await?;

                let precompiled_image_digest = Digest::from_str(&precompiled_content.digest)?;

                let wasm_layer_descriptor = DescriptorBuilder::default()
                    .media_type(&**media_type)
                    .size(bytes.len() as u64)
                    .digest(precompiled_image_digest)
                    .build()?;

                layers_for_runtime.push(WasmLayer {
                    config: wasm_layer_descriptor,
                    layer: bytes.clone(),
                });

                let _ = precompiled_content.lease.release().await;
            }

            return Ok((layers_for_runtime, platform));
        };

        log::info!("using OCI layers");
        Ok((layers, platform))
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn read_wasm_layer(
        &self,
        original_config: &oci_spec::image::Descriptor,
        can_precompile: bool,
        precompile_id: &String,
        needs_precompile: &mut bool,
        media_type_label: &String,
        all_layers: &mut HashMap<Digest, WasmLayer>,
    ) -> std::prelude::v1::Result<(), ShimError> {
        let parent_digest = original_config.digest().clone();
        let digests_to_load = if can_precompile {
            let info = self.get_info(&parent_digest).await?;
            let child_digests = info
                .labels
                .into_iter()
                .filter_map(|(key, child_digest)| {
                    if key.starts_with(&format!("{precompile_id}/child")) {
                        log::debug!("layer {parent_digest} has child layer: {child_digest} ");
                        Some(child_digest)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            if child_digests.is_empty() {
                vec![parent_digest.clone()]
            } else {
                child_digests
                    .into_iter()
                    .map(|d| d.parse().map_err(ShimError::Oci))
                    .collect::<Result<Vec<Digest>>>()?
            }
        } else {
            vec![parent_digest]
        };

        for digest_to_load in digests_to_load {
            if all_layers.contains_key(&digest_to_load) {
                log::debug!("layer {digest_to_load} already loaded");
                continue;
            }
            log::debug!("loading digest: {digest_to_load}");

            let info = self.get_info(&digest_to_load).await?;
            let config_descriptor = match info.labels.get(media_type_label) {
                Some(media_type) => DescriptorBuilder::default()
                    .media_type(&**media_type)
                    .size(info.size as u64)
                    .digest(digest_to_load.clone())
                    .build()?,
                None => original_config.clone(),
            };

            let res = self
                .read_content(&digest_to_load)
                .await
                .map(|module| WasmLayer {
                    config: config_descriptor,
                    layer: module,
                });

            let wasm_layer = match res {
                Ok(res) => res,
                Err(err) if digest_to_load == *original_config.digest() => return Err(err),
                Err(err) => {
                    log::error!("failed to load precompiled layer: {err}");
                    log::error!("falling back to original layer and marking for recompile");
                    *needs_precompile = can_precompile; // only mark for recompile if engine is capable
                    self.read_content(original_config.digest())
                        .await
                        .map(|module| WasmLayer {
                            config: original_config.clone(),
                            layer: module,
                        })?
                }
            };

            all_layers.insert(digest_to_load, wasm_layer);
        }

        Ok(())
    }
}

fn precompile_label(name: &str, version: &str) -> String {
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
    use std::collections::HashSet;
    use std::future::Future;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Arc;

    use oci_tar_builder::WASM_LAYER_MEDIA_TYPE;
    use rand::prelude::*;

    use super::*;
    use crate::container::RuntimeContext;
    use crate::testing::oci_helpers::ImageContent;
    use crate::testing::{oci_helpers, TEST_NAMESPACE};

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
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE)
            .await
            .unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);
        let engine = FakePrecompilerEngine::new(None);

        let (layers, _) = client.load_modules(container_name, &engine).await.unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_bytes.bytes);
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_precompiled_once() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).await.unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecompilerEngine::new(Some(()));
        engine.add_precompiled_bits(&[fake_bytes.bytes.clone()], &fake_precompiled_bytes);

        let (_, _) = client.load_modules(&container_name, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);

        // Even on second calls should only pre-compile once
        let (layers, _) = client.load_modules(&container_name, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_recompiled_if_version_changes() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).await.unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecompilerEngine::new(Some(()));
        engine.add_precompiled_bits(&[fake_bytes.bytes.clone()], &fake_precompiled_bytes);

        let (_, _) = client.load_modules(&container_name, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);

        engine.precompile_id = Some("new_version".to_string());
        let (_, _) = client.load_modules(&container_name, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_precompiled() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).await.unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecompilerEngine::new(Some(()));
        engine.add_precompiled_bits(&[fake_bytes.bytes.clone()], &fake_precompiled_bytes);

        let precompiled_content_digest =
            format!("sha256:{}", digest(fake_precompiled_bytes.bytes.clone()));

        let expected_label = format!(
            "containerd.io/gc.ref.content.precompile.{}",
            precompiled_content_digest
        );

        let (layers, _) = client.load_modules(container_name, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);

        let (manifest, _) = client
            .get_image_manifest_and_digest(&image_name)
            .await
            .unwrap();
        let original_config = manifest.layers().first().unwrap();
        let info = client.get_info(original_config.digest()).await.unwrap();

        let actual_digest = info.labels.get(&expected_label).unwrap();
        assert_eq!(
            actual_digest.to_string(),
            format!("sha256:{}", &digest(fake_precompiled_bytes.bytes.clone()))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_precompiled_but_not_for_all_layers() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).await.unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let non_wasm_bytes = generate_content("original_dont_compile", "textfile");
        let (_image_name, container_name, _cleanup) =
            generate_test_container(None, &[&fake_bytes, &non_wasm_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecompilerEngine::new(Some(()));
        engine.add_precompiled_bits(&[fake_bytes.bytes.clone()], &fake_precompiled_bytes);

        let (layers, _) = client.load_modules(container_name, &engine).await.unwrap();

        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(engine.layers_compiled_per_call.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_do_not_need_precompiled_if_new_layers_are_added_to_existing_image() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).await.unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecompilerEngine::new(Some(()));
        engine.add_precompiled_bits(&[fake_bytes.bytes.clone()], &fake_precompiled_bytes);

        let (layers, _) = client.load_modules(container_name, &engine).await.unwrap();
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
        engine.add_precompiled_bits(&[fake_bytes2.bytes.clone()], &fake_precompiled_bytes2);

        // When a new image with the same name is create the older image content will disappear
        // but since these layers are part of the new image we don't want to have to recompile
        // for the test, let the original image get removed (which would remove any associated content)
        // and then check that the layers don't need to be recompiled
        oci_helpers::wait_for_content_removal(&image_sha).unwrap();

        let (layers, _) = client.load_modules(container_name2, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 2);
        assert_eq!(layers.len(), 1); // Only 1 new layer should be precompiled and returned
        assert_eq!(engine.layers_compiled_per_call.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_do_not_need_precompiled_if_new_layers_are_add_to_new_image() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).await.unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(None, &[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine = FakePrecompilerEngine::new(Some(()));
        engine.add_precompiled_bits(&[fake_bytes.bytes.clone()], &fake_precompiled_bytes);

        let (layers, _) = client.load_modules(container_name, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);

        let fake_bytes2 = generate_content("image2", WASM_LAYER_MEDIA_TYPE);
        let (_image_name2, container_name2, _cleanup2) =
            generate_test_container(None, &[&fake_bytes, &fake_bytes2]);
        let fake_precompiled_bytes2 = generate_content("precompiled2", WASM_LAYER_MEDIA_TYPE);
        engine.add_precompiled_bits(&[fake_bytes2.bytes.clone()], &fake_precompiled_bytes2);

        let (layers, _) = client.load_modules(container_name2, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 2);
        assert_eq!(layers.len(), 1); // Only 1 new layer should be precompiled and returned
        assert_eq!(engine.layers_compiled_per_call.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_layers_are_precompiled_for_multiple_layers() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).await.unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let fake_bytes2 = generate_content("original1", WASM_LAYER_MEDIA_TYPE);

        let (image_name, container_name, _cleanup) =
            generate_test_container(None, &[&fake_bytes, &fake_bytes2]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let fake_precompiled_bytes2 = generate_content("precompiled1", WASM_LAYER_MEDIA_TYPE);

        let mut engine = FakePrecompilerEngine::new(Some(()));
        engine.add_precompiled_bits(&[fake_bytes.bytes.clone()], &fake_precompiled_bytes);
        engine.add_precompiled_bits(&[fake_bytes2.bytes.clone()], &fake_precompiled_bytes2);

        let (layers, _) = client.load_modules(container_name, &engine).await.unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(engine.layers_compiled_per_call.load(Ordering::SeqCst), 2);

        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
        assert_eq!(layers[1].layer, fake_precompiled_bytes2.bytes);

        let (manifest, _) = client
            .get_image_manifest_and_digest(&image_name)
            .await
            .unwrap();

        let precompiled_content_digest1 =
            format!("sha256:{}", digest(fake_precompiled_bytes.bytes.clone()));

        let expected_parent_precompile_label1 = format!(
            "containerd.io/gc.ref.content.precompile.{}",
            precompiled_content_digest1
        );

        let original_config1 = manifest.layers().first().unwrap();
        let info1 = client.get_info(original_config1.digest()).await.unwrap();
        let actual_digest1 = info1
            .labels
            .get(&expected_parent_precompile_label1)
            .unwrap();
        assert_eq!(actual_digest1.to_string(), precompiled_content_digest1,);

        let precompiled_content_digest2 =
            format!("sha256:{}", digest(fake_precompiled_bytes2.bytes.clone()));

        let expected_parent_precompile_label2 = format!(
            "containerd.io/gc.ref.content.precompile.{}",
            precompiled_content_digest2
        );

        let original_config2 = manifest.layers().last().unwrap();
        let info2 = client.get_info(original_config2.digest()).await.unwrap();
        let actual_digest2 = info2
            .labels
            .get(&expected_parent_precompile_label2)
            .unwrap();
        assert_eq!(actual_digest2.to_string(), precompiled_content_digest2,);
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
        let x: u32 = random();
        x
    }

    #[derive(Clone)]
    struct FakePrecompilerEngine {
        precompile_id: Option<String>,
        // precompiled_layers: HashMap<String, Vec<u8>>,
        precompiled_layers: Vec<PrecompiledLayer>,
        precompile_called: Arc<AtomicI32>,
        layers_compiled_per_call: Arc<AtomicI32>,
    }

    impl FakePrecompilerEngine {
        fn new(can_precompile: Option<()>) -> Self {
            let precompile_id = match can_precompile {
                Some(_) => {
                    let precompile_id = format!("uuid-{}", random_number());
                    Some(precompile_id)
                }
                None => None,
            };

            FakePrecompilerEngine {
                precompile_id,
                precompiled_layers: Vec::new(),
                precompile_called: Arc::new(AtomicI32::new(0)),
                layers_compiled_per_call: Arc::new(AtomicI32::new(0)),
            }
        }
        fn add_precompiled_bits(
            &mut self,
            parents: &[Vec<u8>],
            precompiled_content: &oci_helpers::ImageContent,
        ) {
            self.precompiled_layers.push(PrecompiledLayer {
                media_type: precompiled_content.media_type.clone(),
                bytes: precompiled_content.bytes.clone(),
                parents: parents
                    .iter()
                    .map(|p| format!("sha256:{}", digest(p)))
                    .collect(),
            });
        }
    }

    impl Engine for FakePrecompilerEngine {
        fn name() -> &'static str {
            "fake"
        }

        fn run_wasi(&self, _ctx: &impl RuntimeContext) -> std::result::Result<i32, anyhow::Error> {
            panic!("not implemented")
        }

        fn can_precompile(&self) -> Option<String> {
            self.precompile_id.clone()
        }

        fn supported_layers_types() -> &'static [&'static str] {
            &[WASM_LAYER_MEDIA_TYPE, "textfile"]
        }

        fn precompile(
            &self,
            layers: &[WasmLayer],
        ) -> impl Future<Output = Result<Vec<PrecompiledLayer>, anyhow::Error>> + Send {
            async move {
                self.layers_compiled_per_call.store(0, Ordering::SeqCst);
                self.precompile_called.fetch_add(1, Ordering::SeqCst);
                let mut already_collected = HashSet::new(); // prevent returning the same precompiled layer multiple times.
                let mut compiled_layers = vec![];
                for layer in layers {
                    if layer.config.media_type().to_string() == *"textfile" {
                        // simulate a layer that can't be precompiled
                        continue;
                    }

                    let key = digest(layer.layer.clone());
                    if self
                        .precompiled_layers
                        .iter()
                        .any(|l| digest(&l.bytes) == key)
                    {
                        // simulate scenario were one of the layers is already compiled
                        continue;
                    }

                    // if the layer's digest is contained within at least one precompiled layer's set
                    // of parents load the "precompiled" layer that was stored as precompiled for this
                    // layer
                    for precompiled_layer in self.precompiled_layers.iter() {
                        let precompiled_layer_digest =
                            format!("sha256:{}", digest(&precompiled_layer.bytes));
                        let parent_key = format!("sha256:{key}");

                        if precompiled_layer.parents.contains(&parent_key)
                            && !already_collected.contains(&precompiled_layer_digest)
                        {
                            compiled_layers.push(precompiled_layer.clone());
                            already_collected.insert(precompiled_layer_digest);
                        }
                    }
                    self.layers_compiled_per_call.fetch_add(1, Ordering::SeqCst);
                }
                Ok(compiled_layers)
            }
        }
    }
}
