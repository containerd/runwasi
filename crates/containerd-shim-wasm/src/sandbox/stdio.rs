use std::fs::File;
use std::io::ErrorKind::NotFound;
use std::io::{Error, Result};
use std::path::Path;
use std::sync::Arc;

use crossbeam::atomic::AtomicCell;

use super::InstanceConfig;
use crate::sys::stdio::*;

#[derive(Default, Clone)]
pub struct Stdio {
    pub stdin: Stdin,
    pub stdout: Stdout,
    pub stderr: Stderr,
}

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
            stdin: cfg.get_stdin().try_into()?,
            stdout: cfg.get_stdout().try_into()?,
            stderr: cfg.get_stderr().try_into()?,
        })
    }
}

#[derive(Clone, Default)]
pub struct StdioStream<const FD: StdioRawFd>(Arc<AtomicCell<Option<File>>>);

impl<const FD: StdioRawFd> StdioStream<FD> {
    pub fn redirect(self) -> Result<()> {
        if let Some(f) = self.0.take() {
            let f = try_into_fd(f)?;
            let _ = unsafe { libc::dup(FD) };
            if unsafe { libc::dup2(f.as_raw_fd(), FD) } == -1 {
                return Err(Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn take(&self) -> Self {
        Self(Arc::new(AtomicCell::new(self.0.take())))
    }
}

impl<P: AsRef<Path>, const FD: StdioRawFd> TryFrom<Option<P>> for StdioStream<FD> {
    type Error = Error;
    fn try_from(path: Option<P>) -> Result<Self> {
        let file = path
            .and_then(|path| match path.as_ref() {
                path if path.as_os_str().is_empty() => None,
                path => Some(path.to_owned()),
            })
            .map(|path| match open_file(path) {
                Err(err) if err.kind() == NotFound => Ok(None),
                Ok(f) => Ok(Some(f)),
                Err(err) => Err(err),
            })
            .transpose()?
            .flatten();

        Ok(Self(Arc::new(AtomicCell::new(file))))
    }
}

pub type Stdin = StdioStream<STDIN_FILENO>;
pub type Stdout = StdioStream<STDOUT_FILENO>;
pub type Stderr = StdioStream<STDERR_FILENO>;
