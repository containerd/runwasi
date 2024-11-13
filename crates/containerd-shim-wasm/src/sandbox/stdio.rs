use std::io::ErrorKind::NotFound;
use std::io::{Error, Result};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use super::InstanceConfig;
use crate::sys::stdio::*;

#[derive(Default, Clone)]
pub struct Stdio {
    pub stdin: Stdin,
    pub stdout: Stdout,
    pub stderr: Stderr,
}

static INITIAL_STDIO: OnceLock<Stdio> = OnceLock::new();

impl Stdio {
    pub fn redirect(self) -> Result<()> {
        self.stdin.redirect()?;
        self.stdout.redirect()?;
        self.stderr.redirect()?;
        Ok(())
    }

    pub fn take(&self) -> Self {
        Self {
            stdin: self.stdin.take(),
            stdout: self.stdout.take(),
            stderr: self.stderr.take(),
        }
    }

    pub fn init_from_cfg(cfg: &InstanceConfig<impl Send + Sync + Clone>) -> Result<Self> {
        Ok(Self {
            stdin: StdioStream::try_from_path(cfg.get_stdin())?,
            stdout: StdioStream::try_from_path(cfg.get_stdout())?,
            stderr: StdioStream::try_from_path(cfg.get_stderr())?,
        })
    }

    pub fn init_from_std() -> Self {
        Self {
            stdin: Stdin::try_from_std().unwrap_or_default(),
            stdout: Stdout::try_from_std().unwrap_or_default(),
            stderr: Stderr::try_from_std().unwrap_or_default(),
        }
    }

    pub fn guard(self) -> impl Drop {
        StdioGuard(self)
    }
}

struct StdioGuard(Stdio);

impl Drop for StdioGuard {
    fn drop(&mut self) {
        let _ = self.0.take().redirect();
    }
}

#[derive(Clone, Default)]
pub struct StdioStream<const FD: StdioRawFd>(Arc<StdioOwnedFd>);

impl<const FD: StdioRawFd> StdioStream<FD> {
    pub fn redirect(self) -> Result<()> {
        if let Some(fd) = self.0.as_raw_fd() {
            // Before any redirection we try to keep a copy of the original stdio
            // to make sure the streams stay open
            INITIAL_STDIO.get_or_init(Stdio::init_from_std);

            if unsafe { libc::dup2(fd, FD) } == -1 {
                return Err(Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn take(&self) -> Self {
        Self(Arc::new(self.0.take()))
    }

    pub fn try_from_std() -> Result<Self> {
        let fd: i32 = unsafe { libc::dup(FD) };
        if fd == -1 {
            return Err(Error::last_os_error());
        }
        Ok(Self(Arc::new(unsafe { StdioOwnedFd::from_raw_fd(fd) })))
    }
}

impl<const FD: StdioRawFd> StdioStream<FD> {
    fn try_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Ok(Self(Arc::default()));
        }

        let fd = match StdioOwnedFd::try_from_path(path) {
            Err(err) if err.kind() == NotFound => Default::default(),
            Err(err) => return Err(err),
            Ok(fd) => fd,
        };

        Ok(Self(Arc::new(fd)))
    }
}

pub type Stdin = StdioStream<STDIN_FILENO>;
pub type Stdout = StdioStream<STDOUT_FILENO>;
pub type Stderr = StdioStream<STDERR_FILENO>;

#[cfg(test)]
mod test {
    use std::fs::File;

    use tempfile::tempdir;

    use super::*;

    /// containerd can send an empty path or a non-existent path
    /// In both these cases we should just assume that the stdio stream was not setup (intentionally)
    /// Any other error is a real error.
    #[test]
    fn test_maybe_open_stdio() -> anyhow::Result<()> {
        // empty path
        let s = Stdout::try_from_path("")?;
        assert!(s.0.take().as_raw_fd().is_none());

        // nonexistent path
        let s = Stdout::try_from_path("/some/nonexistent/path")?;
        assert!(s.0.take().as_raw_fd().is_none());

        // valid path
        let dir = tempdir()?;
        let path = dir.path().join("testfile");
        let temp = File::create(&path)?;
        drop(temp);

        // a valid path should not fail
        let s = Stdout::try_from_path(path)?;
        assert!(s.0.take().as_raw_fd().is_some());
        Ok(())
    }
}
