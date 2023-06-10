use std::{env, fs};

use {
    anyhow::Context, clap::Parser, oci_spec::image as spec, oci_tar_builder::Builder,
    std::fs::File, std::path::PathBuf,
};

pub fn main() {
    let args = Args::parse();

    let out_dir;
    if let Some(out_path) = args.out_path.as_deref() {
        out_dir = PathBuf::from(out_path);
    } else {
        out_dir = env::current_dir().unwrap();
    }

    let mut builder = Builder::default();

    if let Some(module_path) = args.module.as_deref() {
        let module_path = PathBuf::from(module_path);
        builder.add_layer_with_media_type(
            &module_path,
            "application/vnd.w3c.wasm.module.v1+wasm".to_string(),
        );
    }

    if let Some(components_path) = args.components.as_deref() {
        let paths = fs::read_dir(components_path).unwrap();

        for path in paths {
            let path = path.unwrap().path();
            let ext = path.extension().unwrap().to_str().unwrap();
            match ext {
                "wasm" => {
                    builder.add_layer_with_media_type(
                        &path,
                        "application/vnd.wasm.component.v1+wasm".to_string(),
                    );
                }
                "yml" => {
                    builder.add_layer_with_media_type(
                        &path,
                        "application/vnd.wasm.component.config.v1+json".to_string(),
                    );
                }
                _ => println!(
                    "Skipping Unknown file type: {:?} with extension {:?}",
                    path,
                    path.extension().unwrap()
                ),
            }
        }
    }

    let config = spec::ConfigBuilder::default()
        .entrypoint(vec!["".to_owned()])
        .build()
        .unwrap();

    let img = spec::ImageConfigurationBuilder::default()
        .config(config)
        .os("wasi")
        .architecture("wasm")
        .rootfs(
            spec::RootFsBuilder::default()
                .diff_ids(vec![])
                .build()
                .unwrap(),
        )
        .build()
        .context("failed to build image configuration")
        .unwrap();

    let full_image_name = args.repo.unwrap_or("localhost:5000".to_string()) + "/" + &args.name;
    builder.add_config(img, full_image_name);

    let p = out_dir.join(args.name + ".tar");
    let f = File::create(p).unwrap();
    builder.build(f).unwrap();
}

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    out_path: Option<String>,

    #[arg(short, long)]
    name: String,

    #[arg(short, long)]
    repo: Option<String>,

    #[arg(short, long)]
    module: Option<String>,

    #[arg(short, long)]
    components: Option<String>,
}
