use std::fs::File;
use std::path::Path;

use anyhow::Error as AnyError;
use cap_std::path::PathBuf;
use oci_spec::runtime::Spec;
use oci_spec::OciSpecError;
use serde_json as json;
use thiserror::Error;

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

pub fn get_args(spec: &Spec) -> &[String] {
    let p = match spec.process() {
        None => return &[],
        Some(p) => p,
    };

    match p.args() {
        None => &[],
        Some(args) => args.as_slice(),
    }
}

pub fn spec_from_file<P: AsRef<Path>>(path: P) -> Result<Spec, Error> {
    let file = File::open(path)?;
    let cfg: Spec = json::from_reader(file)?;
    Ok(cfg)
}
