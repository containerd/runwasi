use std::fs::{File, OpenOptions};
use std::io::Result;
use std::path::Path;

pub fn open(path: impl AsRef<Path>) -> Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}
