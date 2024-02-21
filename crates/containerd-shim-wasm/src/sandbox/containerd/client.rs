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
    InfoRequest, ReadContentRequest, UpdateImageRequest, UpdateRequest, WriteAction,
    WriteContentRequest,
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
        original_digest: String,
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
            labels.insert(label.to_string(), original_digest.clone());
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

    fn get_info(&self, content_digest: String) -> Result<Info> {
        self.rt.block_on(async {
            let req = InfoRequest {
                digest: content_digest.clone(),
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

    fn update_image(&self, image: Image) -> Result<Image> {
        self.rt.block_on(async {
            let req = UpdateImageRequest {
                image: Some(image.clone()),
                update_mask: Some(FieldMask {
                    paths: vec!["labels".to_string()],
                }),
            };

            let req = with_namespace!(req, self.namespace);
            let image = ImagesClient::new(self.inner.clone())
                .update(req)
                .await
                .map_err(|err| ShimError::Containerd(err.to_string()))?
                .into_inner()
                .image
                .ok_or_else(|| {
                    ShimError::Containerd(format!("failed to update image {}", image.name))
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

    // load module will query the containerd store to find an image that has an OS of type 'wasm'
    // If found it continues to parse the manifest and return the layers that contains the WASM modules
    // and possibly other configuration layers.
    pub fn load_modules<T: Engine>(
        &self,
        containerd_id: impl ToString,
        engine: &T,
    ) -> Result<(Vec<oci::WasmLayer>, Platform)> {
        let container = self.get_container(containerd_id.to_string())?;
        let mut image = self.get_image(container.image)?;
        let image_digest = self.extract_image_content_sha(&image)?;
        let manifest = self.read_content(image_digest.clone())?;
        let manifest = manifest.as_slice();
        let manifest = ImageManifest::from_reader(manifest)?;

        let image_config_descriptor = manifest.config();
        let image_config = self.read_content(image_config_descriptor.digest())?;
        let image_config = image_config.as_slice();

        // the only part we care about here is the platform values
        let platform: Platform = serde_json::from_slice(image_config)?;
        let Arch::Wasm = platform.architecture() else {
            log::info!("manifest is not in WASM OCI image format");
            return Ok((vec![], platform));
        };

        log::info!("found manifest with WASM OCI image format.");
        // This label is unique across runtimes and version of the shim running
        // a precompiled component/module will not work across different runtimes or versions
        let (can_precompile, precompile_id) = match engine.can_precompile() {
            Some(precompile_id) => (true, precompile_label(T::name(), &precompile_id)),
            None => (false, "".to_string()),
        };

        let needs_precompile = can_precompile && !image.labels.contains_key(&precompile_id);
        let layers = manifest
            .layers()
            .iter()
            .filter(|x| is_wasm_layer(x.media_type(), T::supported_layers_types()))
            .map(|config| {
                
                let mut digest = config.digest().clone();
                if can_precompile {
                    let info = self.get_info(config.digest().clone())?;
                    if info.labels.contains_key(&precompile_id) {
                        log::info!("found precompiled layer in cache: {} ", &precompile_id);
                        digest = info.labels.get(&precompile_id).unwrap().clone();
                    }
                }
                self.read_content(digest).map(|module| WasmLayer {
                    config: config.clone(),
                    layer: module,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        if layers.is_empty() {
            log::info!("no WASM modules found in OCI layers");
            return Ok((vec![], platform));
        }

        if needs_precompile {
            log::info!("precompiling layers for image: {}", image.name);
            
            let mut precompiled_layers = Vec::new();
            for (i, layer) in layers.iter().enumerate() {
                let precompiled_layer = match engine.precompile(layer) {
                    Some(it) => {
                        let precompiled_layer = it?;
                        precompiled_layers.push(WasmLayer {
                            config: layer.config.clone(),
                            layer: precompiled_layer.clone(),
                        });
                        precompiled_layer
                    },
                    None =>{
                        // skip layers that don't support precompilation, and add them to the list of precompiled layers
                         precompiled_layers.push(layer.clone()); 
                         continue
                        } 
                };

                let precompiled_content = self.save_content(precompiled_layer, image_digest.clone(), &precompile_id)?;

                log::debug!("updating image with indicator that precompiled content is available");
                image.labels
                    .insert(precompile_id.clone(), "true".to_string());
                self.update_image(image.clone())?;

                // The original image is considered a root object, by adding a ref to the new compiled content
                // We tell containerd to not garbage collect the new content until this image is removed from the system
                // this ensures that we keep the content around after the lease is dropped
                log::debug!("updating content with precompile digest to avoid garbage collection");
                let mut image_content = self.get_info(image_digest.clone())?;
                image_content.labels.insert(
                    format!("containerd.io/gc.ref.content.precompile.{}",i),
                    precompiled_content.digest.clone(),
                );
                self.update_info(image_content)?;
            }

            return Ok((
                precompiled_layers,
                platform,
            ));
        }

        log::info!("using module from OCI layers");
        Ok((layers, platform))
    }
}

fn precompile_label(name: &str, version: &str) -> String {
    format!("{}/{}/{}", PRECOMPILE_PREFIX, name, version)
}

fn is_wasm_layer(media_type: &MediaType, supported_layer_types: &[&str]) -> bool {
    supported_layer_types.contains(&media_type.to_string().as_str())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
        let returned = client
            .save_content(data, "original".to_string(), &label)
            .unwrap();
        assert_eq!(expected, returned.digest.clone());

        let data = client.read_content(returned.digest.clone()).unwrap();
        assert_eq!(data, b"hello world");

        client
            .save_content(data.clone(), "original".to_string(), &label)
            .expect_err("Should not be able to save when lease is open");

        // need to drop the lease to be able to create a second one
        // a second call should be successful since it already exists
        drop(returned);

        // a second call should be successful since it already exists
        let returned = client
            .save_content(data, "original".to_string(), &label)
            .unwrap();
        assert_eq!(expected, returned.digest);

        client.delete_content(expected.clone()).unwrap();

        client
            .read_content(expected)
            .expect_err("content should not exist");
    }
}
