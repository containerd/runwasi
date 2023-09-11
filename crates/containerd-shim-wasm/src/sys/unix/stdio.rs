use std::fs::OpenOptions;
use std::io::Result;
use std::os::fd::{IntoRawFd, OwnedFd, RawFd};
use std::path::Path;

use crossbeam::atomic::AtomicCell;
pub use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};

pub type StdioRawFd = RawFd;

pub struct StdioOwnedFd(AtomicCell<StdioRawFd>);

impl Drop for StdioOwnedFd {
    fn drop(&mut self) {
        let fd = self.0.swap(-1);
        if fd >= 0 {
            unsafe { libc::close(fd) };
        }
    }
}

impl Default for StdioOwnedFd {
    fn default() -> Self {
        Self(AtomicCell::new(-1))
    }
}

impl StdioOwnedFd {
    pub fn try_from(f: impl Into<OwnedFd>) -> Result<Self> {
        let fd = f.into().into_raw_fd();
        Ok(unsafe { Self::from_raw_fd(fd) })
    }

    pub unsafe fn from_raw_fd(fd: StdioRawFd) -> Self {
        Self(AtomicCell::new(fd))
    }

    pub fn as_raw_fd(&self) -> Option<StdioRawFd> {
        let fd = self.0.load();
        (fd >= 0).then_some(fd)
    }

    pub fn take(&self) -> Self {
        let fd = self.0.swap(-1);
        unsafe { Self::from_raw_fd(fd) }
    }

    pub fn try_from_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::try_from(OpenOptions::new().read(true).write(true).open(path)?)
    }
}
