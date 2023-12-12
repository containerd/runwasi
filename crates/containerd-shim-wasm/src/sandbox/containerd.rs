#![cfg(unix)]

use std::collections::HashMap;

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

use crate::container::{Engine, RuntimeInfo};
use crate::sandbox::error::{Error as ShimError, Result};
use crate::sandbox::oci::{self, WasmLayer};

pub(crate) struct Client {
    inner: Channel,
    rt: Runtime,
    namespace: String,
    address: String,
}

// Adds lease info to grpc header
// https://github.com/containerd/containerd/blob/8459273f806e068e1a6bacfaf1355bbbad738d5e/docs/garbage-collection.md#using-grpc
#[macro_export]
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

struct LeaseGuard {
    lease_id: String,
    namespace: String,
    address: String,
}

pub(crate) struct WriteContent {
    _lease: LeaseGuard,
    pub digest: String,
}

// Provides a best effort for dropping a lease of the content.  If the lease cannot be dropped, it will log a warning
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let id = self.lease_id.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let client = rt.block_on(containerd_client::connect(self.address.clone()));

        if client.is_err() {
            log::warn!("failed to connect to containerd. lease may not be deleted");
            return;
        }

        let mut client = LeasesClient::new(client.unwrap());

        rt.block_on(async {
            let req = containerd_client::services::v1::DeleteRequest { id, sync: false };
            let req = with_namespace!(req, self.namespace);
            let result = client.delete(req).await;

            if result.is_err() {
                log::warn!("failed to remove lease.");
            }
        });
    }
}

// sync wrapper implementation from https://tokio.rs/tokio/topics/bridging
impl Client {
    // wrapper around connection that will establish a connection and create a client
    pub fn connect(address: &str, namespace: impl ToString) -> Result<Client> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let inner = rt
            .block_on(containerd_client::connect(address))
            .map_err(|err| ShimError::Containerd(err.to_string()))?;

        Ok(Client {
            inner,
            rt,
            namespace: namespace.to_string(),
            address: address.to_string(),
        })
    }

    // wrapper around read that will read the entire content file
    pub fn read_content(&self, digest: impl ToString) -> Result<Vec<u8>> {
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
    fn lease(&self, r#ref: String) -> Result<LeaseGuard> {
        self.rt.block_on(async {
            let mut lease_labels = HashMap::new();
            let expire = chrono::Utc::now() + chrono::Duration::hours(24);
            lease_labels.insert("containerd.io/gc.expire".to_string(), expire.to_rfc3339());
            let lease_request = containerd_client::services::v1::CreateRequest {
                id: r#ref.clone(),
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
                    ShimError::Containerd(format!("unable to create lease for  {}", r#ref.clone()))
                })?;

            Ok(LeaseGuard {
                lease_id: lease.id.clone(),
                address: self.address.clone(),
                namespace: self.namespace.clone(),
            })
        })
    }

    pub fn save_content(
        &self,
        data: Vec<u8>,
        original_digest: String,
        info: &RuntimeInfo,
    ) -> Result<WriteContent> {
        let expected = digest(data.clone());
        let expected = format!("sha256:{}", expected);
        let r#ref = format!("precompile-{}-{}-{}", info.name, info.version, expected);
        let lease = self.lease(r#ref.clone())?;

        let digest = self.rt.block_on(async {
            // create a channel to feed the stream; only sending one message at a time so we can set this to one
            let (tx, rx) = mpsc::channel(1);

            let len = data.len() as i64;

            let mut client = ContentClient::new(self.inner.clone());

            // Send write request with Stat action to containerd to let it know that we are going to write content
            // if the content is already there, it will return early with AlreadyExists
            let req = WriteContentRequest {
                r#ref: r#ref.clone(),
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
                        expected.clone()
                    ))
                })?;

            // Write and commit at same time
            let mut labels = HashMap::new();
            labels.insert(
                "runwasi.io/precompiled".to_string(),
                original_digest.clone(),
            );
            let commit_request = WriteContentRequest {
                action: WriteAction::Commit.into(),
                total: len,
                offset: response.offset,
                expected: expected.clone(),
                labels,
                data,
                ..Default::default()
            };
            tx.send(commit_request)
                .await
                .map_err(|err| ShimError::Containerd(err.to_string()))?;
            let response = response_stream
                .message()
                .await
                .map_err(|e| ShimError::Containerd(e.to_string()))?
                .ok_or_else(|| {
                    ShimError::Containerd(format!(
                        "no response received after write request for {}",
                        expected.clone()
                    ))
                })?;

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

    pub fn get_info(&self, content_digest: String) -> Result<Info> {
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

    pub fn update_info(&self, info: Info) -> Result<Info> {
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

    pub fn get_image(&self, image_name: impl ToString) -> Result<Image> {
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

    pub fn update_image(&self, image: Image) -> Result<Image> {
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

    pub fn extract_image_content_sha(&self, image: &Image) -> Result<String> {
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

    pub fn get_container(&self, container_name: impl ToString) -> Result<Container> {
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
        engine: T,
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
        // This label is unique across runtimes and versions
        // a precompiled component/module will not work across different runtimes or versions
        let label = format!(
            "runwasi.io/precompiled/{}/{}",
            T::info().name,
            T::info().version
        );
        match image.labels.get(&label) {
            Some(precompile_digest) if T::can_precompile() => {
                log::info!("found precompiled image");
                let precompiled = self.read_content(precompile_digest)?;
                Ok((
                    vec![WasmLayer {
                        config: image_config_descriptor.clone(),
                        layer: precompiled,
                        precompiled: true,
                    }],
                    platform,
                ))
            }
            None if T::can_precompile() => {
                log::info!("precompiling module");
                let layers = manifest
                    .layers()
                    .iter()
                    .filter(|x| is_wasm_layer(x.media_type(), T::supported_layers_types()))
                    .map(|config| self.read_content(config.digest()))
                    .collect::<Result<Vec<_>>>()?;

                log::debug!("precompile complete and saving content");
                let precompiled = engine.precompile(layers.as_slice())?;
                let precompiled_content =
                    self.save_content(precompiled.clone(), image_digest.clone(), T::info())?;

                log::debug!("updating image with compiled content digest");
                image.labels.insert(
                    "runwasi.io/precompiled".to_string(),
                    precompiled_content.digest.clone(),
                );
                self.update_image(image)?;

                // The original image is considered a root object, by adding a ref to the new compiled content
                // We tell to containerd to not garbage collect the new content until this image is removed from the system
                // this ensures that we keep the content around after the lease is dropped
                log::debug!("updating content with precompile digest to avoid garbage collection");
                let mut image_content = self.get_info(image_digest.clone())?;
                image_content.labels.insert(
                    "containerd.io/gc.ref.content.precompile".to_string(),
                    precompiled_content.digest.clone(),
                );
                self.update_info(image_content)?;

                Ok((
                    vec![WasmLayer {
                        config: image_config_descriptor.clone(),
                        layer: precompiled,
                        precompiled: true,
                    }],
                    platform,
                ))
            }
            _ => {
                log::info!("using module from OCI layers");
                let layers = manifest
                    .layers()
                    .iter()
                    .filter(|x| is_wasm_layer(x.media_type(), T::supported_layers_types()))
                    .map(|config| {
                        self.read_content(config.digest()).map(|module| WasmLayer {
                            config: config.clone(),
                            layer: module,
                            precompiled: false,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok((layers, platform))
            }
        }
    }
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

        let returned = client
            .save_content(
                data,
                "original".to_string(),
                &RuntimeInfo {
                    name: "test",
                    version: "0.0.0",
                },
            )
            .unwrap();
        assert_eq!(expected, returned.digest.clone());

        let data = client.read_content(returned.digest.clone()).unwrap();
        assert_eq!(data, b"hello world");

        // need to drop the lease to be able to create a second one
        drop(returned);

        // a second call should be successful since it already exists
        let returned = client
            .save_content(
                data,
                "original".to_string(),
                &RuntimeInfo {
                    name: "test",
                    version: "0.0.0",
                },
            )
            .unwrap();
        assert_eq!(expected, returned.digest);

        client.delete_content(expected.clone()).unwrap();

        client
            .read_content(expected)
            .expect_err("content should not exist");
    }
}
