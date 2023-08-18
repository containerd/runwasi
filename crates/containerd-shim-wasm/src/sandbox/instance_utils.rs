//! Common utilities for the containerd shims.
use std::{
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

use crate::sandbox::error::Error;

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

#[cfg(test)]
mod tests {
    use std::fs::File;

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
}
