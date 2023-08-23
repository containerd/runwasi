//! Common utilities for the containerd shims.
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::Error;

/// Return the root path for the instance.
///
/// The root path is the path to the directory containing the container's state.
pub fn get_instance_root<P: AsRef<Path>>(
    root_path: P,
    instance_id: &str,
) -> Result<PathBuf, anyhow::Error> {
    let instance_root = construct_instance_root(root_path, instance_id)?;
    if !instance_root.exists() {
        bail!("container {} does not exist.", instance_id)
    }
    Ok(instance_root)
}

/// Checks if the container exists.
///
/// The root path is the path to the directory containing the container's state.
pub fn instance_exists<P: AsRef<Path>>(root_path: P, container_id: &str) -> Result<bool> {
    let instance_root = construct_instance_root(root_path, container_id)?;
    Ok(instance_root.exists())
}

/// containerd can send an empty path or a non-existant path
/// In both these cases we should just assume that the stdio stream was not setup (intentionally)
/// Any other error is a real error.
pub fn maybe_open_stdio(path: &str) -> Result<Option<File>, Error> {
    if path.is_empty() {
        return Ok(None);
    }
    match OpenOptions::new().read(true).write(true).open(path) {
        Ok(f) => Ok(Some(f)),
        Err(err) => match err.kind() {
            ErrorKind::NotFound => Ok(None),
            _ => Err(err.into()),
        },
    }
}

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

pub fn determine_rootdir<P: AsRef<Path>>(
    bundle: P,
    namespace: &str,
    rootdir: P,
) -> Result<PathBuf, Error> {
    let file = match File::open(bundle.as_ref().join("options.json")) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(rootdir.as_ref().join(namespace)),
        Err(e) => return Err(e.into()),
    };
    let path = serde_json::from_reader::<_, Options>(file)?
        .root
        .unwrap_or_else(|| rootdir.as_ref().to_owned())
        .join(namespace);
    log::info!("container runtime root path is {path:?}");
    Ok(path)
}

fn construct_instance_root<P: AsRef<Path>>(root_path: P, container_id: &str) -> Result<PathBuf> {
    let root_path = fs::canonicalize(&root_path).with_context(|| {
        format!(
            "failed to canonicalize {} for container {}",
            root_path.as_ref().display(),
            container_id
        )
    })?;
    Ok(root_path.join(container_id))
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use std::fs::{File, OpenOptions};
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_maybe_open_stdio() -> Result<(), Error> {
        let f = maybe_open_stdio("")?;
        assert!(f.is_none());

        let f = maybe_open_stdio("/some/nonexistent/path")?;
        assert!(f.is_none());

        let dir = tempdir()?;
        let temp = File::create(dir.path().join("testfile"))?;
        drop(temp);
        let f = maybe_open_stdio(dir.path().join("testfile").as_path().to_str().unwrap())?;
        assert!(f.is_some());
        Ok(())
    }

    #[test]
    fn test_determine_rootdir_with_options_file() -> Result<(), Error> {
        let namespace = "test_namespace";
        let dir = tempdir()?;
        let rootdir = dir.path().join("runwasi");
        let opts = Options {
            root: Some(rootdir.clone()),
        };
        let opts_file = OpenOptions::new()
            .read(true)
            .create(true)
            .truncate(true)
            .write(true)
            .open(dir.path().join("options.json"))?;
        write!(&opts_file, "{}", serde_json::to_string(&opts)?)?;
        let root = determine_rootdir(
            dir.path(),
            namespace,
            &PathBuf::from("/run/containerd/runtime"),
        )?;
        assert_eq!(root, rootdir.join(namespace));
        Ok(())
    }

    #[test]
    fn test_determine_rootdir_without_options_file() -> Result<(), Error> {
        let dir = tempdir()?;
        let namespace = "test_namespace";
        let root = determine_rootdir(
            dir.path(),
            namespace,
            &PathBuf::from("/run/containerd/runtime"),
        )?;
        assert!(root.is_absolute());
        assert_eq!(
            root,
            PathBuf::from("/run/containerd/runtime").join(namespace)
        );
        Ok(())
    }
}

#[cfg(test)]
mod rootdirtest {}
