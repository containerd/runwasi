use std::fs::File;
use std::io::ErrorKind::NotFound;
use std::io::{Error, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::sys::stdio::*;

#[derive(Default, Clone)]
pub struct Stdio {
    pub stdin: Stdin,
    pub stdout: Stdout,
    pub stderr: Stderr,
}

impl Stdio {
    pub fn redirect(&self) -> Result<()> {
        self.stdin.redirect()?;
        self.stdout.redirect()?;
        self.stderr.redirect()?;
        Ok(())
    }
}

macro_rules! stdio_impl {
    ( $stdio_type:ident, $fd:expr ) => {
        #[derive(Default, Clone)]
        pub struct $stdio_type(Arc<Mutex<Option<File>>>);

        impl<P: AsRef<Path>> TryFrom<Option<P>> for $stdio_type {
            type Error = std::io::Error;
            fn try_from(path: Option<P>) -> Result<Self> {
                path.and_then(|path| match path.as_ref() {
                    path if path.as_os_str().is_empty() => None,
                    path => Some(path.to_owned()),
                })
                .map(|path| match open_file(path) {
                    Err(err) if err.kind() == NotFound => Ok(None),
                    Ok(f) => Ok(Some(f)),
                    Err(err) => Err(err),
                })
                .transpose()
                .map(|opt| Self(Arc::new(Mutex::new(opt.flatten()))))
            }
        }

        impl Drop for $stdio_type {
            fn drop(&mut self) {
                if let Some(f) = self.0.try_lock().ok().and_then(|mut f| f.take()) {
                    unsafe {
                        let _ = libc::close(f.as_raw_fd());
                    }
                }
            }
        }

        impl $stdio_type {
            pub fn redirect(&self) -> Result<()> {
                if let Some(f) = self.0.try_lock().ok().and_then(|mut f| f.take()) {
                    let f = try_into_fd(f)?;
                    let _ = unsafe { libc::dup($fd) };
                    if unsafe { libc::dup2(f.as_raw_fd(), $fd) } == -1 {
                        return Err(Error::last_os_error());
                    }
                }
                Ok(())
            }
        }
    };
}

stdio_impl!(Stdin, STDIN_FILENO);
stdio_impl!(Stdout, STDOUT_FILENO);
stdio_impl!(Stderr, STDERR_FILENO);
