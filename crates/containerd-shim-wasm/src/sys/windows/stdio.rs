use std::io::ErrorKind::Other;
use std::io::{Error, Result};
use std::os::windows::prelude::{AsRawHandle, IntoRawHandle, OwnedHandle};

use libc::{c_int, close, intptr_t, open_osfhandle, O_APPEND};

type StdioRawFd = libc::c_int;

pub static STDIN_FILENO: StdioRawFd = 0;
pub static STDOUT_FILENO: StdioRawFd = 1;
pub static STDERR_FILENO: StdioRawFd = 2;

struct StdioOwnedFd(c_int);

pub fn try_into_fd(f: impl Into<OwnedHandle>) -> Result<impl StdioAsRawFd> {
    let handle = f.into();
    let fd = unsafe { open_osfhandle(handle.as_raw_handle() as intptr_t, O_APPEND) };
    if fd == -1 {
        return Err(Error::new(Other, "Failed to open file descriptor"));
    }
    let _ = handle.into_raw_handle(); // drop ownership of the handle, it's managed by fd now
    Ok(StdioOwnedFd(fd))
}

pub trait StdioAsRawFd {
    fn as_raw_fd(&self) -> c_int;
}

impl StdioAsRawFd for StdioOwnedFd {
    fn as_raw_fd(&self) -> c_int {
        self.0
    }
}

impl Drop for StdioOwnedFd {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}
