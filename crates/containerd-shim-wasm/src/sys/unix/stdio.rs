use std::io::Result;
pub use std::os::fd::AsRawFd as StdioAsRawFd;
use std::os::fd::OwnedFd;

pub use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};

pub fn try_into_fd(f: impl Into<OwnedFd>) -> Result<impl StdioAsRawFd> {
    Ok(f.into())
}
