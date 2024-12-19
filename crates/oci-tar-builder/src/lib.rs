use std::collections::HashMap;
use std::fs::metadata;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Error, Result};
use indexmap::IndexMap;
use log::{debug, warn};
use oci_spec::image::{
    DescriptorBuilder, Digest, ImageConfiguration, ImageIndexBuilder, ImageManifestBuilder,
    MediaType, PlatformBuilder, SCHEMA_VERSION,
};
use oci_wasm::{WasmConfig, WASM_ARCHITECTURE};
use serde::Serialize;
use sha256::{digest, try_digest};
#[derive(Debug)]
pub struct Builder<C: OciConfig> {
    configs: Vec<(C, String, MediaType)>,
    layers: Vec<(PathBuf, String)>,
}

pub trait OciConfig {
    fn os(&self) -> String;
    fn architecture(&self) -> String;
    fn layers(&self) -> Vec<String>;
    fn to_string(&self) -> String;
}

impl OciConfig for ImageConfiguration {
    fn os(&self) -> String {
        self.os().to_string()
    }

    fn architecture(&self) -> String {
        self.architecture().to_string()
    }

    fn layers(&self) -> Vec<String> {
        self.rootfs().diff_ids().to_vec()
    }

    fn to_string(&self) -> String {
        self.to_string_pretty().unwrap()
    }
}

impl OciConfig for WasmConfig {
    fn os(&self) -> String {
        self.os.to_string()
    }

    fn architecture(&self) -> String {
        WASM_ARCHITECTURE.to_string()
    }

    fn layers(&self) -> Vec<String> {
        self.layer_digests.clone()
    }

    fn to_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap()
    }
}

impl Default for Builder<WasmConfig> {
    fn default() -> Self {
        Self {
            configs: Vec::new(),
            layers: Vec::new(),
        }
    }
}

impl Default for Builder<ImageConfiguration> {
    fn default() -> Self {
        Self {
            configs: Vec::new(),
            layers: Vec::new(),
        }
    }
}

#[derive(Serialize, Debug)]
struct OciLayout {
    #[serde(rename = "imageLayoutVersion")]
    image_layout_version: String,
}

impl Default for OciLayout {
    fn default() -> Self {
        Self {
            image_layout_version: "1.0.0".to_string(),
        }
    }
}

#[derive(Serialize, Debug)]
struct DockerManifest {
    #[serde(rename = "Config")]
    config: String,
    #[serde(rename = "RepoTags")]
    repo_tags: Vec<String>,
    #[serde(rename = "Layers")]
    layers: Vec<String>,
}

pub const WASM_LAYER_MEDIA_TYPE: &str =
    "application/vnd.bytecodealliance.wasm.component.layer.v0+wasm";

impl<C: OciConfig> Builder<C> {
    pub fn add_config(&mut self, config: C, name: String, media_type: MediaType) -> &mut Self {
        self.configs.push((config, name, media_type));
        self
    }

    pub fn add_layer(&mut self, layer: &PathBuf) -> &mut Self {
        self.layers.push((layer.to_owned(), "".to_string()));
        self
    }

    pub fn add_layer_with_media_type(&mut self, layer: &PathBuf, media_type: String) -> &mut Self {
        self.layers.push((layer.to_owned(), media_type));
        self
    }

    pub fn build<W: Write>(&mut self, w: W) -> Result<(), Error> {
        let mut tb = tar::Builder::new(w);
        let mut manifests = Vec::new();
        // use IndexMap in order to keep layers in order they were added.
        let mut layer_digests = IndexMap::new();

        if self.configs.len() > 1 {
            anyhow::bail!("only one config is supported");
        }

        let mut mfst = DockerManifest {
            config: "".to_string(),
            repo_tags: Vec::new(),
            layers: Vec::new(),
        };

        for layer in self.layers.iter() {
            let dgst = try_digest(layer.0.as_path()).context("failed to digest layer")?;
            let meta = metadata(layer.0.clone()).context("could not get layer metadata")?;

            let mut media_type = MediaType::ImageLayer;
            if !layer.1.is_empty() {
                media_type = MediaType::Other(layer.1.clone());
            }
            let desc = DescriptorBuilder::default()
                // TODO: check file headers to determine mediatype? Could also just require it to be passed in on add_layer
                .media_type(media_type)
                .digest(Digest::try_from(format!("sha256:{dgst}"))?)
                .size(meta.len())
                .build()
                .context("failed to build descriptor")?;
            layer_digests.insert(format!("sha256:{dgst}"), desc);

            let mut th = tar::Header::new_gnu();
            th.set_mode(0o444);
            th.set_size(meta.len());
            let p = "blobs/sha256/".to_owned() + &dgst;
            th.set_path(&p).context("could not set path for layer")?;
            th.set_cksum();
            let f = std::fs::File::open(layer.0.clone()).context("could not open layer")?;
            tb.append(&th, f)?;

            mfst.layers.push(p.to_string());
        }

        for config in self.configs.iter() {
            let s = config.0.to_string();
            let b = s.as_bytes();
            let dgst = digest(b);
            let mut th = tar::Header::new_gnu();
            th.set_mode(0o444);
            th.set_size(b.len() as u64);
            let p = "blobs/sha256/".to_owned() + &dgst;
            th.set_path(&p).context("could not set path for config")?;
            th.set_cksum();
            tb.append(&th, b)?;

            mfst.config = p.to_string();

            let desc = DescriptorBuilder::default()
                .media_type(config.2.clone())
                .size(b.len() as u64)
                .digest(Digest::try_from(format!("sha256:{dgst}"))?)
                .build()
                .context("failed to build descriptor")?;

            // add all layer_digests including any OCI WASM types that are may not be in the rootfs
            let mut layers = Vec::new();
            for (_k, v) in layer_digests.iter() {
                layers.push(v.clone());
            }

            for id in config.0.layers().iter() {
                debug!("id: {}", id);
                if layer_digests.get(id).is_none() {
                    warn!("rootfs diff with id {} not found in layers", id);
                }
            }

            let mut annotations = HashMap::new();
            if config.1.contains(':') {
                let split = config.1.split(':').collect::<Vec<&str>>()[1];
                annotations.insert(
                    "org.opencontainers.image.ref.name".to_string(),
                    split.to_string(),
                );
            }
            mfst.repo_tags.push(config.1.clone());
            annotations.insert("io.containerd.image.name".to_string(), config.1.clone());

            let manifest = ImageManifestBuilder::default()
                .schema_version(SCHEMA_VERSION)
                .media_type(MediaType::ImageManifest);

            let manifest = manifest
                .layers(layers)
                .config(desc)
                .annotations(annotations.clone())
                .build()
                .context("failed to build manifest")?
                .to_string()
                .context("failed to serialize manifest")?;
            let b = manifest.as_bytes();
            let dgst = digest(b);

            let mut th = tar::Header::new_gnu();
            th.set_mode(0o444);
            th.set_size(b.len() as u64);
            th.set_path("blobs/sha256/".to_owned() + &dgst)
                .context("could not set path for manifest")?;
            th.set_cksum();
            tb.append(&th, b)?;

            let platform = PlatformBuilder::default()
                .os(config.0.os().as_str())
                .architecture(config.0.architecture().as_str())
                .build()
                .context("failed to build platform")?;

            let desc = DescriptorBuilder::default()
                .media_type(MediaType::ImageManifest)
                .size(b.len() as u64)
                .platform(platform)
                .annotations(annotations)
                .digest(Digest::try_from(format!("sha256:{dgst}"))?)
                .build()
                .context("failed to build descriptor")?;

            manifests.push(desc);
        }

        let idx = ImageIndexBuilder::default()
            .schema_version(SCHEMA_VERSION)
            .media_type(MediaType::ImageIndex)
            .manifests(manifests)
            .build()
            .context("failed to build index")?;

        let s = idx.to_string().context("failed to serialize index")?;
        let b = s.as_bytes();

        let mut th = tar::Header::new_gnu();
        th.set_path("index.json")
            .context("could not set path to index.json")?;
        th.set_size(b.len() as u64);
        th.set_mode(0o644);
        th.set_cksum();

        tb.append(&th, b)?;

        let layout = serde_json::to_string(&OciLayout::default())
            .context("failed to serialize oci-layout")?;
        let b = layout.as_bytes();

        let mut th = tar::Header::new_gnu();
        th.set_path("oci-layout")
            .context("could not set path for oci-layout file")?;
        th.set_size(b.len() as u64);
        th.set_mode(0o644);
        th.set_cksum();
        tb.append(&th, b)?;

        let mfst_data =
            serde_json::to_string(&vec![&mfst]).context("failed to serialize manifest")?;
        let mut th = tar::Header::new_gnu();
        th.set_path("manifest.json")?;
        th.set_mode(0o644);
        th.set_size(mfst_data.as_bytes().len() as u64);
        th.set_cksum();
        tb.append(&th, mfst_data.as_bytes())?;

        tb.finish()?;

        Ok(())
    }
}
