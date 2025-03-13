use std::fs::{File, OpenOptions};
use std::io::Result;
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;

pub fn open(path: impl AsRef<Path>) -> Result<File> {
    // Containerd always passes a named pipe for stdin, stdout, and stderr so we can check if it is a pipe and open with overlapped IO
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if path.as_ref().starts_with(r"\\.\pipe\") {
        options.custom_flags(FILE_FLAG_OVERLAPPED);
    }
    options.open(path)
}
