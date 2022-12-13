use anyhow::{Context, Error, Result};
use log::debug;
use oci_spec::image::{
    DescriptorBuilder, ImageConfiguration, ImageIndexBuilder, ImageManifestBuilder, MediaType,
    PlatformBuilder, SCHEMA_VERSION,
};
use serde::Serialize;
use sha256::{digest, try_digest};
use std::collections::HashMap;
use std::fs::metadata;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct Builder {
    configs: Vec<(ImageConfiguration, String)>,
    layers: Vec<PathBuf>,
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

impl Builder {
    pub fn add_config(&mut self, config: ImageConfiguration, name: String) -> &mut Self {
        self.configs.push((config, name));
        return self;
    }

    pub fn add_layer(&mut self, layer: &PathBuf) -> &mut Self {
        self.layers.push(layer.to_owned());
        return self;
    }

    pub fn build<W: Write>(&mut self, w: W) -> Result<(), Error> {
        let mut tb = tar::Builder::new(w);
        let mut manifests = Vec::new();
        let mut layer_digests = HashMap::new();

        if self.configs.len() > 1 {
            anyhow::bail!("only one config is supported");
        }

        let mut mfst = DockerManifest {
            config: "".to_string(),
            repo_tags: Vec::new(),
            layers: Vec::new(),
        };

        for layer in self.layers.iter() {
            let dgst = try_digest(layer.as_path()).context("failed to digest layer")?;
            let meta = metadata(layer).context("could not get layer metadata")?;
            let oci_digest = "sha256:".to_owned() + &dgst;

            let desc = DescriptorBuilder::default()
                // TODO: check file headers to determine mediatype? Could also just require it to be passed in on add_layer
                .media_type(MediaType::ImageLayer)
                .digest(&oci_digest)
                .size(meta.len() as i64)
                .build()
                .context("failed to build descriptor")?;
            layer_digests.insert(oci_digest, desc);

            let mut th = tar::Header::new_gnu();
            th.set_mode(0o444);
            th.set_size(meta.len() as u64);
            let p = "blobs/sha256/".to_owned() + &dgst;
            th.set_path(&p).context("could not set path for layer")?;
            th.set_cksum();
            let f = std::fs::File::open(layer).context("could not open layer")?;
            tb.append(&th, f)?;

            mfst.layers.push(p.to_string());
        }

        for config in self.configs.iter() {
            let s = config.0.to_string().context("failed to serialize config")?;
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
                .media_type(MediaType::ImageConfig)
                .size(b.len() as i64)
                .digest("sha256:".to_owned() + &dgst)
                .build()
                .context("failed to build descriptor")?;

            let mut layers = Vec::new();
            for id in config.0.rootfs().diff_ids().iter() {
                debug!("id: {}", id);
                layer_digests.get(id).map(|d| layers.push(d.clone()));
            }

            let mut annotations = HashMap::new();
            if config.1.contains(":") {
                let split = config.1.split(":").collect::<Vec<&str>>()[1];
                annotations.insert(
                    "org.opencontainers.image.ref.name".to_string(),
                    split.to_string(),
                );
            }
            mfst.repo_tags.push(config.1.clone());
            annotations.insert("io.containerd.image.name".to_string(), config.1.clone());

            let manifest = ImageManifestBuilder::default()
                .schema_version(SCHEMA_VERSION)
                .media_type(MediaType::ImageManifest)
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
                .os(config.0.os().clone())
                .architecture(config.0.architecture().clone())
                .build()
                .context("failed to build platform")?;

            let desc = DescriptorBuilder::default()
                .media_type(MediaType::ImageManifest)
                .size(b.len() as i64)
                .platform(platform)
                .annotations(annotations)
                .digest("sha256:".to_owned() + &dgst)
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
