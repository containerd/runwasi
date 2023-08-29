use std::fs::{File, OpenOptions};
use std::io::Result;
use std::os::fd::OwnedFd;
pub use std::os::fd::{AsRawFd as StdioAsRawFd, RawFd as StdioRawFd};
use std::path::Path;

pub use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};

pub fn try_into_fd(f: impl Into<OwnedFd>) -> Result<impl StdioAsRawFd> {
    Ok(f.into())
}

pub fn open_file<P: AsRef<Path>>(path: P) -> Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}
