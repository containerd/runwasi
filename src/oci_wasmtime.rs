use std::fs::OpenOptions;
use std::path::Path;

use anyhow::Context;
use cap_std::fs::File as CapFile;
use containerd_shim_wasm::sandbox::{error::Error, oci};
use oci_spec::runtime::Spec;
use wasmtime_wasi::sync::file::File as WasiFile;
use wasmtime_wasi::{Dir as WasiDir, WasiCtxBuilder};

pub fn get_rootfs(spec: &Spec) -> Result<WasiDir, Error> {
    let path = oci::get_root(spec).to_str().unwrap();
    let rootfs = wasi_dir(path, OpenOptions::new().read(true))?;
    Ok(rootfs)
}

pub fn env_to_wasi(spec: &Spec) -> Vec<(String, String)> {
    let default = vec![];
    let env = spec
        .process()
        .as_ref()
        .unwrap()
        .env()
        .as_ref()
        .unwrap_or(&default);
    let mut vec: Vec<(String, String)> = Vec::with_capacity(env.len());

    for v in env {
        match v.split_once('=') {
            None => vec.push((v.to_string(), "".to_string())),
            Some(t) => vec.push((t.0.to_string(), t.1.to_string())),
        };
    }

    vec
}

pub fn spec_to_wasi<P: AsRef<Path>>(
    builder: WasiCtxBuilder,
    bundle_path: P,
    spec: &mut Spec,
) -> Result<WasiCtxBuilder, Error> {
    spec.canonicalize_rootfs(bundle_path)?;
    let root = match spec.root() {
        Some(r) => r.path().to_str().unwrap(),
        None => return Err(Error::InvalidArgument("rootfs is not set".to_string())),
    };

    let rootfs = match wasi_dir(root, OpenOptions::new().read(true)) {
        Ok(r) => r,
        Err(e) => {
            return Err(Error::InvalidArgument(format!(
                "could not open rootfs: {0}",
                e
            )));
        }
    };

    let args = oci::get_args(spec);
    if args.is_empty() {
        return Err(Error::InvalidArgument("args is not set".to_string()));
    }

    let env = env_to_wasi(spec);
    let builder = builder
        .preopened_dir(rootfs, "/")
        .context("could not set rootfs")?
        .envs(env.as_slice())
        .context("could not set envs")?
        .args(args)
        .context("could not set command args")?;

    Ok(builder)
}

pub fn wasi_dir(path: &str, opts: &OpenOptions) -> Result<WasiDir, std::io::Error> {
    let f = opts.open(path)?;
    Ok(WasiDir::from_std_file(f))
}

pub fn wasi_file<P: AsRef<Path>>(
    path: P,
    opts: &mut OpenOptions,
) -> Result<WasiFile, std::io::Error> {
    let f = opts.open(path)?;
    Ok(WasiFile::from_cap_std(CapFile::from_std(f)))
}
