use super::{Error, Result};
use std::process::{Command, Stdio};

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
