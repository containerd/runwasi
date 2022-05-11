use anyhow::{Context, Error as AnyError};
use cap_std::fs::File as CapFile;
use cap_std::path::PathBuf;
use oci_spec::runtime::Spec;
use oci_spec::OciSpecError;
use serde_json as json;
use std::fs::{File, OpenOptions};
use std::path::Path;
use thiserror::Error;
use wasmtime_wasi::sync::file::File as WasiFile;
use wasmtime_wasi::{Dir as WasiDir, WasiCtxBuilder};

#[derive(Error, Debug)]
pub enum Error {
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("failed to load spec: {0}")]
    Spec(#[from] OciSpecError),
    #[error("failed to open file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to decode spec json: ${0}")]
    Json(#[from] json::Error),
    #[error("{0}")]
    Any(#[from] AnyError),
}

pub fn load(path: &str) -> Result<Spec, Error> {
    let spec = Spec::load(path)?;
    Ok(spec)
}

pub fn get_root(spec: &Spec) -> &PathBuf {
    let root = spec.root().as_ref().unwrap();
    root.path()
}

pub fn get_rootfs(spec: &Spec) -> Result<WasiDir, Error> {
    let path = get_root(spec).to_str().unwrap();
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
        match v.split_once("=") {
            None => vec.push((v.to_string(), "".to_string())),
            Some(t) => vec.push((t.0.to_string(), t.1.to_string())),
        };
    }

    return vec;
}

pub fn get_args(spec: &Spec) -> &[String] {
    let p = match spec.process() {
        None => return &[],
        Some(p) => p,
    };

    match p.args() {
        None => &[],
        Some(args) => return args.as_slice(),
    }
}

pub fn spec_from_file<P: AsRef<Path>>(path: P) -> Result<Spec, Error> {
    let file = File::open(path)?;
    let cfg: Spec = json::from_reader(file)?;
    return Ok(cfg);
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

    let args = get_args(spec);
    if args.len() == 0 {
        return Err(Error::InvalidArgument("args is not set".to_string()));
    }

    let env = env_to_wasi(&spec);
    let builder = builder
        .preopened_dir(rootfs, "/")
        .context("could not set rootfs")?
        .envs(env.as_slice())
        .context("could not set envs")?
        .args(args)
        .context("could not set command args")?;

    return Ok(builder);
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
