use super::{Error, Result};
use std::process::Command;

pub fn run_test_with_sudo(test: &str) -> Result<()> {
    Command::new("sudo")
        .arg("-E")
        .arg(std::fs::read_link("/proc/self/exe")?)
        .arg("--")
        .arg(test)
        .arg("--exact")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(std::io::ErrorKind::Other.into())
            }
        })
        .map_err(Error::from)
}

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

pub(crate) use function;
