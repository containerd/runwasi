#![cfg(unix)]

use std::collections::HashMap;
use std::path::Path;

use containerd_client;
use containerd_client::services::v1::containers_client::ContainersClient;
use containerd_client::services::v1::content_client::ContentClient;
use containerd_client::services::v1::images_client::ImagesClient;
use containerd_client::services::v1::leases_client::LeasesClient;
use containerd_client::services::v1::{
    Container, DeleteContentRequest, GetContainerRequest, GetImageRequest, Image, Info,
    InfoRequest, ReadContentRequest, UpdateRequest, WriteAction, WriteContentRequest,
};
use containerd_client::tonic::transport::Channel;
use containerd_client::{tonic, with_namespace};
use futures::TryStreamExt;
use oci_spec::image::{Arch, ImageManifest, MediaType, Platform};
use prost_types::FieldMask;
use sha256::digest;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request};

use super::lease::LeaseGuard;
use crate::container::Engine;
use crate::sandbox::error::{Error as ShimError, Result};
use crate::sandbox::oci::{self, WasmLayer};
use crate::with_lease;

static PRECOMPILE_PREFIX: &str = "runwasi.io/precompiled";

pub struct Client {
    inner: Channel,
    rt: Runtime,
    namespace: String,
    address: String,
}

#[derive(Debug)]
pub(crate) struct WriteContent {
    _lease: LeaseGuard,
    pub digest: String,
}

// sync wrapper implementation from https://tokio.rs/tokio/topics/bridging
impl Client {
    // wrapper around connection that will establish a connection and create a client
    pub fn connect(
        address: impl AsRef<Path> + ToString,
        namespace: impl ToString,
    ) -> Result<Client> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let inner = rt
            .block_on(containerd_client::connect(address.as_ref()))
            .map_err(|err| ShimError::Containerd(err.to_string()))?;

        Ok(Client {
            inner,
            rt,
            namespace: namespace.to_string(),
            address: address.to_string(),
        })
    }

    // wrapper around read that will read the entire content file
    fn read_content(&self, digest: impl ToString) -> Result<Vec<u8>> {
        self.rt.block_on(async {
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
        })
    }

    // used in tests to clean up content
    #[allow(dead_code)]
    fn delete_content(&self, digest: impl ToString) -> Result<()> {
        self.rt.block_on(async {
            let req = DeleteContentRequest {
                digest: digest.to_string(),
            };
            let req = with_namespace!(req, self.namespace);
            ContentClient::new(self.inner.clone())
                .delete(req)
                .await
                .map_err(|err| ShimError::Containerd(err.to_string()))?;
            Ok(())
        })
    }

    // wrapper around lease that will create a lease and return a guard that will delete the lease when dropped
    fn lease(&self, reference: String) -> Result<LeaseGuard> {
        self.rt.block_on(async {
            let mut lease_labels = HashMap::new();
            let expire = chrono::Utc::now() + chrono::Duration::hours(24);
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

            Ok(LeaseGuard {
                lease_id: lease.id,
                address: self.address.clone(),
                namespace: self.namespace.clone(),
            })
        })
    }

    fn save_content(
        &self,
        data: Vec<u8>,
        original_digest: &str,
        label: &str,
    ) -> Result<WriteContent> {
        let expected = format!("sha256:{}", digest(data.clone()));
        let reference = format!("precompile-{}", label);
        let lease = self.lease(reference.clone())?;

        let digest = self.rt.block_on(async {
            // create a channel to feed the stream; only sending one message at a time so we can set this to one
            let (tx, rx) = mpsc::channel(1);

            let len = data.len() as i64;
            log::debug!("Writing {} bytes to content store", len);
            let mut client = ContentClient::new(self.inner.clone());

            // Send write request with Stat action to containerd to let it know that we are going to write content
            // if the content is already there, it will return early with AlreadyExists
            log::debug!("Sending stat request to containerd");
            let req = WriteContentRequest {
                r#ref: reference.clone(),
                action: WriteAction::Stat.into(),
                total: len,
                expected: expected.clone(),
                ..Default::default()
            };
            tx.send(req)
                .await
                .map_err(|err| ShimError::Containerd(err.to_string()))?;
            let request_stream = ReceiverStream::new(rx);
            let request_stream =
                with_lease!(request_stream, self.namespace, lease.lease_id.clone());
            let mut response_stream = match client.write(request_stream).await {
                Ok(response_stream) => response_stream.into_inner(),
                Err(e) if e.code() == Code::AlreadyExists => {
                    log::info!("content already exists {}", expected.clone().to_string());
                    return Ok(expected);
                }
                Err(e) => return Err(ShimError::Containerd(e.to_string())),
            };
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

            // There is a scenario where the content might have been removed manually
            // but the content isn't removed from the containerd file system yet.
            // In this case if we re-add it at before its removed from file system
            // we don't need to copy the content again.  Container tells us it found the blob
            // by returning the offset of the content that was found.
            let data_to_write = data[response.offset as usize..].to_vec();

            // Write and commit at same time
            let mut labels = HashMap::new();
            labels.insert(format!("{}/original", label), original_digest.to_string());
            let commit_request = WriteContentRequest {
                action: WriteAction::Commit.into(),
                total: len,
                offset: response.offset,
                expected: expected.clone(),
                labels,
                data: data_to_write,
                ..Default::default()
            };
            log::debug!(
                "Sending commit request to containerd with response: {:?}",
                response
            );
            tx.send(commit_request)
                .await
                .map_err(|err| ShimError::Containerd(format!("commit request error: {}", err)))?;
            let response = response_stream
                .message()
                .await
                .map_err(|err| ShimError::Containerd(format!("response stream error: {}", err)))?
                .ok_or_else(|| {
                    ShimError::Containerd(format!(
                        "no response received after write request for {}",
                        expected.clone()
                    ))
                })?;

            log::debug!("Validating response");
            // client should validate that all bytes were written and that the digest matches
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
            Ok(response.digest)
        })?;

        Ok(WriteContent {
            _lease: lease,
            digest: digest.clone(),
        })
    }

    fn get_info(&self, content_digest: &str) -> Result<Info> {
        self.rt.block_on(async {
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
                    ShimError::Containerd(format!(
                        "failed to get info for content {}",
                        content_digest
                    ))
                })?;
            Ok(info)
        })
    }

    fn update_info(&self, info: Info) -> Result<Info> {
        self.rt.block_on(async {
            let req = UpdateRequest {
                info: Some(info.clone()),
                update_mask: Some(FieldMask {
                    paths: vec!["labels".to_string()],
                }),
            };
            let req = with_namespace!(req, self.namespace);
            let info = ContentClient::new(self.inner.clone())
                .update(req)
                .await
                .map_err(|err| ShimError::Containerd(err.to_string()))?
                .into_inner()
                .info
                .ok_or_else(|| {
                    ShimError::Containerd(format!(
                        "failed to update info for content {}",
                        info.digest
                    ))
                })?;
            Ok(info)
        })
    }

    fn get_image(&self, image_name: impl ToString) -> Result<Image> {
        self.rt.block_on(async {
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
        })
    }

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

    fn get_container(&self, container_name: impl ToString) -> Result<Container> {
        self.rt.block_on(async {
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
        })
    }

    fn get_image_manifest(&self, image_name: &str) -> Result<(ImageManifest, String)> {
        let image = self.get_image(image_name)?;
        let image_digest = self.extract_image_content_sha(&image)?;
        let manifest = self.read_content(&image_digest)?;
        let manifest = manifest.as_slice();
        let manifest = ImageManifest::from_reader(manifest)?;
        Ok((manifest, image_digest))
    }

    // load module will query the containerd store to find an image that has an OS of type 'wasm'
    // If found it continues to parse the manifest and return the layers that contains the WASM modules
    // and possibly other configuration layers.
    pub fn load_modules<T: Engine>(
        &self,
        containerd_id: impl ToString,
        engine: &T,
    ) -> Result<(Vec<oci::WasmLayer>, Platform)> {
        let container = self.get_container(containerd_id.to_string())?;
        let (manifest, image_digest) = self.get_image_manifest(&container.image)?;

        let image_config_descriptor = manifest.config();
        let image_config = self.read_content(image_config_descriptor.digest())?;
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

        let image_info = self.get_info(&image_digest)?;
        let mut needs_precompile =
            can_precompile && !image_info.labels.contains_key(&precompile_id);
        let layers = manifest
            .layers()
            .iter()
            .filter(|x| is_wasm_layer(x.media_type(), T::supported_layers_types()))
            .map(|original_config| {
                let mut digest_to_load = original_config.digest().clone();
                if can_precompile {
                    let info = self.get_info(&digest_to_load)?;
                    if info.labels.contains_key(&precompile_id) {
                        // Safe to unwrap here since we already checked for the label's existence
                        digest_to_load = info.labels.get(&precompile_id).unwrap().clone();
                        log::info!(
                            "layer {} has pre-compiled content: {} ",
                            info.digest,
                            &digest_to_load
                        );
                    }
                }
                log::debug!("loading digest: {} ", &digest_to_load);
                self.read_content(&digest_to_load)
                    .map(|module| WasmLayer {
                        config: original_config.clone(),
                        layer: module,
                    })
                    .or_else(|e| {
                        // handle content being removed from the content store out of band
                        if digest_to_load != *original_config.digest() {
                            log::error!("failed to load precompiled layer: {}", e);
                            log::error!("falling back to original layer and marking for recompile");
                            needs_precompile = can_precompile; // only mark for recompile if engine is capable
                            self.read_content(original_config.digest())
                                .map(|module| WasmLayer {
                                    config: original_config.clone(),
                                    layer: module,
                                })
                        } else {
                            Err(e)
                        }
                    })
            })
            .collect::<Result<Vec<_>>>()?;

        if needs_precompile {
            log::info!("precompiling layers for image: {}", container.image);
            match engine.precompile(&layers) {
                Some(compiled_layer_result) => {
                    let compiled_layers = compiled_layer_result?;

                    for (i, precompiled_layer) in compiled_layers.iter().enumerate() {
                        let original_layer = &layers[i];

                        let precompiled_content = self.save_content(
                            precompiled_layer.layer.clone(),
                            original_layer.config.digest(),
                            &precompile_id,
                        )?;

                        log::debug!(
                            "updating original layer {} with compiled layer {}",
                            original_layer.config.digest(),
                            precompiled_content.digest
                        );
                        let mut original_layer = self.get_info(original_layer.config.digest())?;
                        original_layer
                            .labels
                            .insert(precompile_id.clone(), precompiled_content.digest.clone());
                        self.update_info(original_layer)?;

                        // The original image is considered a root object, by adding a ref to the new compiled content
                        // We tell containerd to not garbage collect the new content until this image is removed from the system
                        // this ensures that we keep the content around after the lease is dropped
                        // We also save the precompiled flag here since the image labels can be mutated containerd, for example if the image is pulled twice
                        log::debug!(
                            "updating image content with precompile digest to avoid garbage collection"
                        );
                        let mut image_content = self.get_info(&image_digest)?;
                        image_content.labels.insert(
                            format!("containerd.io/gc.ref.content.precompile.{}", i),
                            precompiled_content.digest,
                        );
                        image_content
                            .labels
                            .insert(precompile_id.clone(), "true".to_string());
                        self.update_info(image_content)?;
                    }
                    return Ok((compiled_layers, platform));
                }
                None => {
                    log::error!("Nothing to use precompiled layers");
                }
            };
        }

        if layers.is_empty() {
            log::info!("no WASM layers found in OCI image");
            return Ok((vec![], platform));
        }

        log::info!("using OCI layers");
        Ok((layers, platform))
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

#[cfg(test)]
mod tests {
    use oci_spec::image::{Config, Descriptor};
    use oci_tar_builder::WASM_LAYER_MEDIA_TYPE;
    use std::fs::write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use rand::prelude::*;


    use crate::container::RuntimeContext;
    use crate::sandbox::Stdio;
    use crate::testing::oci_helpers::ImageContent;
    use crate::testing::{oci_helpers, TEST_NAMESPACE};

    use super::*;

    #[test]
    fn test_save_content() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, "test-ns").unwrap();
        let data = b"hello world".to_vec();

        let expected = digest(data.clone());
        let expected = format!("sha256:{}", expected);

        let label = precompile_label("test", "hasdfh");
        let returned = client.save_content(data, "original", &label).unwrap();
        assert_eq!(expected, returned.digest.clone());

        let data = client.read_content(returned.digest.clone()).unwrap();
        assert_eq!(data, b"hello world");

        client
            .save_content(data.clone(), "original", &label)
            .expect_err("Should not be able to save when lease is open");

        // need to drop the lease to be able to create a second one
        // a second call should be successful since it already exists
        drop(returned);

        // a second call should be successful since it already exists
        let returned = client.save_content(data, "original", &label).unwrap();
        assert_eq!(expected, returned.digest);

        client.delete_content(expected.clone()).unwrap();

        client
            .read_content(expected)
            .expect_err("content should not exist");
    }

    #[test]
    fn test_layers_when_precompile_not_supported() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, TEST_NAMESPACE).unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_, container_name, _cleanup) = generate_test_container(&[&fake_bytes]);
        let engine = fake_precompiler_engine::new(None);

        let (layers, _) = client.load_modules(&container_name, &engine).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_bytes.bytes);
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_layers_are_precompiled_once() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE).unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(&[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine =
            fake_precompiler_engine::new(Some(()));
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);

        let (_, _) = client.load_modules(&container_name, &engine).unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);

        // Even on second calls should only pre-compile once
        let (layers, _) = client.load_modules(&container_name, &engine).unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
    }

    #[test]
    fn test_layers_are_recompiled_if_version_changes() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE).unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (_image_name, container_name, _cleanup) = generate_test_container(&[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine =
            fake_precompiler_engine::new(Some(()));
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);

        let (_, _) = client.load_modules(&container_name, &engine).unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);

        engine.precompile_id = Some("new_version".to_string());
        let (_, _) = client.load_modules(&container_name, &engine).unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_layers_are_precompiled() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE).unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let (image_name, container_name, _cleanup) = generate_test_container(&[&fake_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut engine =
            fake_precompiler_engine::new(Some(()));
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);
            let expected_id = precompile_label(fake_precompiler_engine::name(), engine.can_precompile().unwrap().as_str());

        let (layers, _) = client.load_modules(&container_name, &engine).unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);

        let (manifest, _) = client.get_image_manifest(&image_name).unwrap();
        let original_config = manifest.layers().first().unwrap();
        let info = client.get_info(original_config.digest()).unwrap();

        let actual_digest = info.labels.get(&expected_id).unwrap();
        assert_eq!(
            actual_digest.to_string(),
            format!("sha256:{}", &digest(fake_precompiled_bytes.bytes.clone()))
        );
    }

    #[test]
    fn test_layers_are_precompiled_but_not_for_all_layers() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE).unwrap();

        let fake_bytes = generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let non_wasm_bytes = generate_content("original_dont_compile", "textfile");
        let (_image_name, container_name, _cleanup) = generate_test_container(&[&fake_bytes, &non_wasm_bytes]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let mut  engine =
            fake_precompiler_engine::new(Some(()));
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);

        let (layers, _) = client.load_modules(&container_name, &engine).unwrap();
        
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
        assert_eq!(layers[1].layer, non_wasm_bytes.bytes);
    }

    #[test]
    fn test_layers_are_precompiled_for_multiple_layers() {
        let path = PathBuf::from("/run/containerd/containerd.sock");
        let path = path.to_str().unwrap();
        let client = Client::connect(path, crate::testing::TEST_NAMESPACE).unwrap();

        let fake_bytes =generate_content("original", WASM_LAYER_MEDIA_TYPE);
        let fake_bytes2 =generate_content("original1", WASM_LAYER_MEDIA_TYPE);

        let (image_name, container_name, _cleanup) =
            generate_test_container(&[&fake_bytes, &fake_bytes2]);

        let fake_precompiled_bytes = generate_content("precompiled", WASM_LAYER_MEDIA_TYPE);
        let fake_precompiled_bytes2 = generate_content("precompiled1", WASM_LAYER_MEDIA_TYPE);

        let mut engine = fake_precompiler_engine::new(
            Some(())
        );
        engine.add_precompiled_bits(fake_bytes.bytes.clone(), &fake_precompiled_bytes);
        engine.add_precompiled_bits(fake_bytes2.bytes.clone(), &fake_precompiled_bytes2);

        let expected_id = precompile_label(fake_precompiler_engine::name(), engine.can_precompile().unwrap().as_str());

        let (layers, _) = client.load_modules(&container_name, &engine).unwrap();
        assert_eq!(engine.precompile_called.load(Ordering::SeqCst), 1);

        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].layer, fake_precompiled_bytes.bytes);
        assert_eq!(layers[1].layer, fake_precompiled_bytes2.bytes);

        let (manifest, _) = client.get_image_manifest(&image_name).unwrap();

        let original_config1 = manifest.layers().first().unwrap();
        let info1 = client.get_info(original_config1.digest()).unwrap();
        let actual_digest1 = info1.labels.get(&expected_id).unwrap();
        assert_eq!(
            actual_digest1.to_string(),
            format!("sha256:{}", &digest(fake_precompiled_bytes.bytes.clone()))
        );

        let original_config2 = manifest.layers().last().unwrap();
        let info2 = client.get_info(original_config2.digest()).unwrap();
        let actual_digest2 = info2.labels.get(&expected_id).unwrap();
        assert_eq!(
            actual_digest2.to_string(),
            format!("sha256:{}", &digest(fake_precompiled_bytes2.bytes.clone()))
        );
    }

    fn generate_test_container(original: &[&oci_helpers::ImageContent]) -> (String, String, oci_helpers::OCICleanup) {
        let _ = env_logger::try_init();

        let random_number = random_number(); 
        let image_name = format!("localhost/test:latest{}", random_number);
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
    struct fake_precompiler_engine {
        precompile_id: Option<String>,
        precompiled_layers: HashMap<String, WasmLayer>,
        precompile_called: Arc<AtomicI32>,
    }

    impl fake_precompiler_engine {
        fn new(can_precompile: Option<()>) -> Self {
            let precompile_id = match can_precompile  {
                Some(_) => {
                    let precompile_id = format!("uuid-{}", random_number());
                    Some(precompile_id)
                },
                None => None,
            };

            fake_precompiler_engine {
                precompile_id: precompile_id,
                precompiled_layers: HashMap::new(),
                precompile_called: Arc::new(AtomicI32::new(0)),
            }
        }
        fn add_precompiled_bits(&mut self, original: Vec<u8>, precompiled_content: &oci_helpers::ImageContent) {
            let key = digest(original);
            
            self.precompiled_layers.insert(key ,WasmLayer {
                config: Descriptor::new(
                    MediaType::Other(precompiled_content.media_type.clone()),
                    precompiled_content.bytes.len() as i64,
                    digest(precompiled_content.bytes.clone()),
                ),
                layer: precompiled_content.bytes.clone(),
            });
        }
    }

   

    impl Engine for fake_precompiler_engine {
        fn name() -> &'static str {
            "fake"
        }

        fn run_wasi(
            &self,
            ctx: &impl RuntimeContext,
            stdio: Stdio,
        ) -> std::result::Result<i32, anyhow::Error> {
            panic!("not implemented")
        }

        fn can_precompile(&self) -> Option<String> {
            return self.precompile_id.clone();
        }

        fn supported_layers_types() -> &'static [&'static str] {
            &[WASM_LAYER_MEDIA_TYPE, "textfile"]
        }

        fn precompile(
            &self,
            layers: &[WasmLayer],
        ) -> std::option::Option<std::result::Result<Vec<WasmLayer>, anyhow::Error>> {
            self.precompile_called.fetch_add(1, Ordering::SeqCst);
            let mut compiled_layers = vec![];
            for layer in layers{

                if layer.config.media_type().to_string() == "textfile".to_string() {
                    // simulate a layer that can't be precompiled
                    compiled_layers.push(layer.clone());
                    continue;
                }

                // load the "precompiled" layer that was stored as precompiled for this layer
                let key = digest(layer.layer.clone());
                let precompiled = self.precompiled_layers[&key].clone();
                compiled_layers.push(precompiled);
            }
            Some(Ok(compiled_layers))
        }
    }
}
