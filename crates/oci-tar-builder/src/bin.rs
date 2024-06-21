use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::{env, fs};

use anyhow::Context;
use clap::Parser;
use oci_spec::image::{self as spec, Arch, ImageConfiguration};
use oci_tar_builder::Builder;
use oci_wasm::WasmConfig;
use sha256::{digest, try_digest};

#[tokio::main]
pub async fn main() {
    let args = Args::parse();

    let out_dir;
    if let Some(out_path) = args.out_path.as_deref() {
        out_dir = PathBuf::from(out_path);
        fs::create_dir_all(out_dir.parent().unwrap()).unwrap();
    } else {
        out_dir = env::current_dir().unwrap();
    }

    if !args.module.is_empty() && args.components.is_some() {
        println!("Mutually exclusive flags: module and components");
        return;
    }

    if args.module.is_empty() && args.components.is_none() {
        println!("Must supply module or components");
        return;
    }

    if args.as_artifact {
        generate_wasm_artifact(args, out_dir).await.unwrap();
    } else {
        generate_wasi_image(args, out_dir).unwrap();
    }
}

async fn generate_wasm_artifact(args: Args, out_dir: PathBuf) -> Result<(), anyhow::Error> {
    println!("Generating wasm artifact");

    let mut builder = Builder::<WasmConfig>::default();

    let (conf, path) = match args.components {
        Some(path) => {
            let paths = fs::read_dir(&path)?;
            if paths.count() != 1 {
                println!(
                    "Currently only supports a single component file {:?}",
                    &path
                );
            }
            let (conf, _) = WasmConfig::from_component(&path, None).await?;
            (conf, path)
        }
        None => {
            let module_path = args.module.first().unwrap();
            let (conf, _) = WasmConfig::from_module(module_path, None).await?;
            (conf, module_path.to_string())
        }
    };

    builder.add_config(
        conf,
        args.repo + "/" + &args.name + ":" + &args.tag,
        spec::MediaType::Other(oci_wasm::WASM_MANIFEST_CONFIG_MEDIA_TYPE.to_string()),
    );

    let module_path = PathBuf::from(path);
    builder.add_layer_with_media_type(&module_path, oci_wasm::WASM_LAYER_MEDIA_TYPE.to_string());

    println!("Creating oci tar file {}", out_dir.clone().display());
    let f = File::create(out_dir.clone())?;
    match builder.build(f) {
        Ok(_) => println!("Successfully created oci tar file {}", out_dir.display()),
        Err(e) => {
            print!(
                "Building oci tar file {} failed: {:?}",
                out_dir.display(),
                e
            );
            fs::remove_file(out_dir).unwrap_or(print!("Failed to clean up oci tar file on error"));
        }
    }

    Ok(())
}

fn generate_wasi_image(args: Args, out_dir: PathBuf) -> Result<(), anyhow::Error> {
    println!("Generating wasm oci image");
    let entry_point = args.name.clone() + ".wasm";

    let mut builder = Builder::<ImageConfiguration>::default();
    let mut layer_digests = Vec::new();
    for module_path in args.module.iter() {
        let module_path = PathBuf::from(module_path);
        builder.add_layer_with_media_type(
            &module_path,
            oci_tar_builder::WASM_LAYER_MEDIA_TYPE.to_string(),
        );

        layer_digests
            .push(try_digest(&module_path).context("failed to calculate digest for module")?);
    }

    for layer_config in args.layer.iter() {
        //split string on equals sign
        let layer_options: Vec<&str> = layer_config.split('=').collect();

        let layer_type = layer_options.first().unwrap();
        let layer_path = PathBuf::from(layer_options.last().unwrap());
        builder.add_layer_with_media_type(&layer_path, layer_type.to_string());
        layer_digests
            .push(try_digest(&layer_path).context("failed to calculate digest for module")?);
    }

    if let Some(components_path) = args.components.as_deref() {
        let paths = fs::read_dir(components_path)?;

        for path in paths {
            let path = path?.path();
            let ext = path
                .extension()
                .unwrap_or(std::ffi::OsStr::new(""))
                .to_str()
                .unwrap_or("");
            match ext {
                "wasm" => {
                    builder.add_layer_with_media_type(
                        &path,
                        oci_tar_builder::WASM_LAYER_MEDIA_TYPE.to_string(),
                    );
                    layer_digests
                        .push(try_digest(&path).context("failed to calculate digest for module")?);
                }
                _ => println!(
                    "Skipping Unknown file type: {:?} with extension {:?}",
                    path,
                    path.extension().unwrap_or(std::ffi::OsStr::new(""))
                ),
            }
        }
    }

    // Need each config to be unique since we don't have layers to make them unique in the rootfs
    // https://github.com/opencontainers/image-spec/pull/1173
    let unique_id = digest(layer_digests.join(""));
    let mut labels: HashMap<String, String> = HashMap::new();
    labels.insert("containerd.runwasi.layers".to_string(), unique_id);

    let config = spec::ConfigBuilder::default()
        .entrypoint(vec![entry_point])
        .labels(labels)
        .build()?;

    let conf = spec::ImageConfigurationBuilder::default()
        .config(config)
        .os("wasip1")
        .architecture(Arch::Wasm)
        .rootfs(spec::RootFsBuilder::default().diff_ids(vec![]).build()?)
        .build()
        .context("failed to build image configuration")?;

    builder.add_config(
        conf,
        format!("{}/{}:{}", args.repo, args.name, args.tag),
        spec::MediaType::ImageConfig,
    );

    println!("Creating oci tar file {}", out_dir.clone().display());
    let f = File::create(out_dir.clone()).unwrap();
    match builder.build(f) {
        Ok(_) => println!("Successfully created oci tar file {}", out_dir.display()),
        Err(e) => {
            print!(
                "Building oci tar file {} failed: {:?}",
                out_dir.display(),
                e
            );
            fs::remove_file(out_dir).unwrap_or(print!("Failed to remove temporary file"));
        }
    }

    Ok(())
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    out_path: Option<String>,

    #[arg(short, long)]
    name: String,

    #[arg(short, long)]
    tag: String,

    #[arg(short, long)]
    repo: String,

    #[arg(short, long)]
    module: Vec<String>,

    #[arg(short, long)]
    layer: Vec<String>,

    #[arg(short, long)]
    components: Option<String>,

    #[arg(short, long)]
    as_artifact: bool,
}
