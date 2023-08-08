//! Testing utilities used across different modules

use std::process::{Command, Stdio};

use super::{Error, Result};

fn normalize_test_name(test: &str) -> Result<&str> {
    let closure_removed = test.trim_end_matches("::{{closure}}");

    // More tests and validation here if needed.

    Ok(closure_removed)
}

/// Re-execs the current process with sudo and runs the given test.
/// Unless this is run in a CI environment, this may prompt the user for a password.
/// This is significantly faster than expecting the user to run the tests with sudo due to build and crate caching.
pub fn run_test_with_sudo(test: &str) -> Result<()> {
    // This uses piped stdout/stderr.
    // This makes it so cargo doesn't mess up the caller's TTY.
    // This also explicitly sets LD_LIBRARY_PATH, which sudo usually removes.
    // This might be needed when dynamically linking libwasmedge.

    let normalized_test = normalize_test_name(test)?;
    let ld_library_path = std::env::var_os("LD_LIBRARY_PATH")
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let mut cmd = Command::new("sudo")
        .arg("-E")
        .arg("env")
        .arg(format!("LD_LIBRARY_PATH={ld_library_path}"))
        .arg(std::env::current_exe().unwrap())
        .arg("--")
        .arg(normalized_test)
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

#[cfg(unix)]
use caps::{CapSet, Capability};
pub use function;

/// Determines if the current process has the CAP_SYS_ADMIN capability in its effective set.
pub fn has_cap_sys_admin() -> bool {
    #[cfg(unix)]
    {
        let caps = caps::read(None, CapSet::Effective).unwrap();
        caps.contains(&Capability::CAP_SYS_ADMIN)
    }

    #[cfg(windows)]
    {
        false
    }
}
