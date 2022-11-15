use std::fs::File;
use std::path::Path;

use super::cgroups;
use super::error::Result;
use cap_std::path::PathBuf;
pub use oci_spec::runtime::Spec;
use serde_json as json;

pub fn load(path: &str) -> Result<Spec> {
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

pub fn spec_from_file<P: AsRef<Path>>(path: P) -> Result<Spec> {
    let file = File::open(path)?;
    let cfg: Spec = json::from_reader(file)?;
    Ok(cfg)
}

struct NopCgroup {}

impl cgroups::Cgroup for NopCgroup {
    fn add_task(&self, _pid: u32) -> Result<()> {
        Ok(())
    }

    fn version(&self) -> cgroups::Version {
        cgroups::Version::V1
    }

    fn apply(&self, _res: Option<cgroups::Resources>) -> Result<()> {
        Ok(())
    }

    fn delete(&self) -> Result<()> {
        Ok(())
    }
}

pub fn get_cgroup(spec: &Spec) -> Result<Box<dyn cgroups::Cgroup>> {
    let linux = spec.linux();
    if linux.is_none() {
        return Ok(Box::new(NopCgroup {}));
    }

    match linux.as_ref().unwrap().cgroups_path() {
        None => Ok(Box::new(NopCgroup {})),
        Some(p) => cgroups::new(p.clone().as_path().to_str().unwrap().to_string()),
    }
}

pub fn setup_cgroup(cg: &dyn cgroups::Cgroup, spec: &Spec) -> Result<()> {
    if let Some(linux) = spec.linux() {
        if let Some(res) = linux.resources() {
            cg.apply(Some(res.clone())).map_err(|e| {
                super::Error::Others(format!(
                    "error applying cgroup settings from oci spec: cgroup version {}: {}",
                    cg.version(),
                    e
                ))
            })?;
        }
    }
    Ok(())
}
