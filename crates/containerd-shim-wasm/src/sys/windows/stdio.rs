use std::fs::{File, OpenOptions};
use std::io::ErrorKind::Other;
use std::io::{Error, Result};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::prelude::{AsRawHandle, IntoRawHandle, OwnedHandle};
use std::path::Path;

use libc::{c_int, close, intptr_t, open_osfhandle, O_APPEND};
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;

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

pub fn open_file<P: AsRef<Path>>(path: P) -> Result<File> {
    // Containerd always passes a named pipe for stdin, stdout, and stderr so we can check if it is a pipe and open with overlapped IO
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if path.as_ref().starts_with("\\\\.\\pipe\\") {
        options.custom_flags(FILE_FLAG_OVERLAPPED);
    }

    options.open(path)
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
