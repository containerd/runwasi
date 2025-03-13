//! Common utilities for the containerd shims.

use std::fs::File;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{Error, InstanceConfig};
use crate::sys::DEFAULT_CONTAINER_ROOT_DIR;
use crate::sys::stdio::open;

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

impl InstanceConfig {
    /// Determine the root directory for the container runtime.
    ///
    /// If the `bundle` directory contains an `options.json` file, the root directory is read from the
    /// file. Otherwise, the root directory is determined by `{DEFAULT_CONTAINER_ROOT_DIR}/{runtime}/{namespace}`.
    ///
    /// The default root directory is `/run/containerd/<wasm engine name>/<namespace>`.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Debug"))]
    pub fn determine_rootdir(
        &self,
        runtime: impl AsRef<str> + std::fmt::Debug,
    ) -> Result<PathBuf, Error> {
        let rootdir = DEFAULT_CONTAINER_ROOT_DIR.join(runtime.as_ref());
        let file = match File::open(self.bundle.join("options.json")) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(rootdir.join(&self.namespace)),
            Err(e) => return Err(e.into()),
        };
        let path = serde_json::from_reader::<_, Options>(file)?
            .root
            .unwrap_or(rootdir)
            .join(&self.namespace);
        log::info!("container runtime root path is {path:?}");
        Ok(path)
    }

    pub fn open_stdin(&self) -> IoResult<File> {
        if self.stdin.as_os_str().is_empty() {
            return Err(IoError::new(ErrorKind::NotFound, "File not found"));
        }
        open(&self.stdin)
    }

    pub fn open_stdout(&self) -> IoResult<File> {
        if self.stdout.as_os_str().is_empty() {
            return Err(IoError::new(ErrorKind::NotFound, "File not found"));
        }
        open(&self.stdout)
    }

    pub fn open_stderr(&self) -> IoResult<File> {
        if self.stderr.as_os_str().is_empty() {
            return Err(IoError::new(ErrorKind::NotFound, "File not found"));
        }
        open(&self.stderr)
    }
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
        let cfg = InstanceConfig {
            bundle: dir.path().to_path_buf(),
            namespace: namespace.to_string(),
            ..Default::default()
        };
        let root = cfg.determine_rootdir("runtime")?;
        assert_eq!(root, rootdir.join(namespace));
        Ok(())
    }

    #[test]
    fn test_determine_rootdir_without_options_file() -> Result<(), Error> {
        let dir = tempdir()?;
        let namespace = "test_namespace";
        let cfg = InstanceConfig {
            bundle: dir.path().to_path_buf(),
            namespace: namespace.to_string(),
            ..Default::default()
        };
        let root = cfg.determine_rootdir("runtime")?;
        assert!(root.is_absolute());
        assert_eq!(
            root,
            PathBuf::from("/run/containerd/runtime").join(namespace)
        );
        Ok(())
    }
}
