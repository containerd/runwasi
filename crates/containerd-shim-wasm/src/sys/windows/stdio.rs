use std::fs::OpenOptions;
use std::io::ErrorKind::Other;
use std::io::{Error, Result};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::prelude::{AsRawHandle, IntoRawHandle, OwnedHandle};
use std::path::Path;

use crossbeam::atomic::AtomicCell;
use libc::{intptr_t, open_osfhandle, O_APPEND};
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;

pub type StdioRawFd = libc::c_int;

pub const STDIN_FILENO: StdioRawFd = 0;
pub const STDOUT_FILENO: StdioRawFd = 1;
pub const STDERR_FILENO: StdioRawFd = 2;

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
    pub fn try_from(f: impl Into<OwnedHandle>) -> Result<Self> {
        let handle = f.into();
        let fd = unsafe { open_osfhandle(handle.as_raw_handle() as intptr_t, O_APPEND) };
        if fd == -1 {
            return Err(Error::new(Other, "Failed to open file descriptor"));
        }
        let _ = handle.into_raw_handle(); // drop ownership of the handle, it's managed by fd now
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
        // Containerd always passes a named pipe for stdin, stdout, and stderr so we can check if it is a pipe and open with overlapped IO
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        if path.as_ref().starts_with(r"\\.\pipe\") {
            options.custom_flags(FILE_FLAG_OVERLAPPED);
        }
        Self::try_from(options.open(path)?)
    }
}
