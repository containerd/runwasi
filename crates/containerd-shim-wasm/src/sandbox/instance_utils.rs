//! Common utilities for the containerd shims.
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Error;

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

#[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
pub fn determine_rootdir(
    bundle: impl AsRef<Path>,
    namespace: &str,
    rootdir: impl AsRef<Path>,
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

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_determine_rootdir_with_options_file() -> Result<(), Error> {
        let namespace = "test_namespace";
        let dir = tempdir()?;
        let rootdir = dir.path().join("runwasi");
        let opts = Options {
            root: Some(rootdir.clone()),
        };
        std::fs::write(
            dir.path().join("options.json"),
            serde_json::to_string(&opts)?,
        )?;
        let root = determine_rootdir(dir.path(), namespace, "/run/containerd/runtime")?;
        assert_eq!(root, rootdir.join(namespace));
        Ok(())
    }

    #[test]
    fn test_determine_rootdir_without_options_file() -> Result<(), Error> {
        let dir = tempdir()?;
        let namespace = "test_namespace";
        let root = determine_rootdir(dir.path(), namespace, "/run/containerd/runtime")?;
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
