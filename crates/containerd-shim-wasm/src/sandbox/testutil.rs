//! Testing utilities used across different modules

use super::{Error, Result};
use std::process::{Command, Stdio};

/// Re-execs the current process with sudo and runs the given test.
/// Unless this is run in a CI environment, this may prompt the user for a password.
/// This is significantly faster than expecting the user to run the tests with sudo due to build and crate caching.
pub fn run_test_with_sudo(test: &str) -> Result<()> {
    // This uses piped stdout/stderr.
    // This makes it so cargo doesn't mess up the caller's TTY.
    let mut cmd = Command::new("sudo")
        .arg("-E")
        .arg(std::fs::read_link("/proc/self/exe")?)
        .arg("--")
        .arg(test)
        .arg("--exact")
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdout = cmd.stdout.take().unwrap();
    let mut stderr = cmd.stderr.take().unwrap();

    std::thread::spawn(move || {
        std::io::copy(&mut stdout, &mut std::io::stdout()).unwrap();
    });
    std::thread::spawn(move || {
        std::io::copy(&mut stderr, &mut std::io::stderr()).unwrap();
    });

    cmd.wait()
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(std::io::ErrorKind::Other.into())
            }
        })
        .map_err(Error::from)
}

#[macro_export]
macro_rules! function {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        let name = &name[..name.len() - 3][env!("CARGO_PKG_NAME").len() + 2..];
        name
    }};
}

pub use function;
