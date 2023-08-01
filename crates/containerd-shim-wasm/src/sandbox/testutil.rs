//! Testing utilities used across different modules

use super::{instance::Wait, EngineGetter, Error, Instance, InstanceConfig, Result};
use anyhow::Result as AnyHowResult;
use chrono::{DateTime, Utc};
use libc::SIGKILL;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::{
    borrow::Cow,
    fs::{create_dir, File, OpenOptions},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc::channel,
    time::Duration,
};
use tempfile::TempDir;

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

    let normalized_test = normalize_test_name(test)?;

    let mut cmd = Command::new("sudo")
        .arg("-E")
        .arg(std::fs::read_link("/proc/self/exe")?)
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
pub use function;

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

pub fn run_wasi_test<I, E>(
    dir: &TempDir,
    wasmbytes: Cow<[u8]>,
    start_fn: Option<&str>,
) -> AnyHowResult<(u32, DateTime<Utc>), Error>
where
    I: Instance<E = E> + EngineGetter<E = E>,
    E: Sync + Send + Clone,
{
    create_dir(dir.path().join("rootfs"))?;
    let rootdir = dir.path().join("runwasi");
    create_dir(&rootdir)?;
    let opts = Options {
        root: Some(rootdir),
    };
    let opts_file = OpenOptions::new()
        .read(true)
        .create(true)
        .truncate(true)
        .write(true)
        .open(dir.path().join("options.json"))?;
    write!(&opts_file, "{}", serde_json::to_string(&opts)?)?;

    let wasm_path = dir.path().join("rootfs/hello.wasm");
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(wasm_path)?;
    f.write_all(&wasmbytes)?;

    let stdout = File::create(dir.path().join("stdout"))?;
    drop(stdout);

    let entrypoint = match start_fn {
        Some(s) => "./hello.wasm#".to_string() + s,
        None => "./hello.wasm".to_string(),
    };
    let spec = SpecBuilder::default()
        .root(RootBuilder::default().path("rootfs").build()?)
        .process(
            ProcessBuilder::default()
                .cwd("/")
                .args(vec![entrypoint])
                .build()?,
        )
        .build()?;

    spec.save(dir.path().join("config.json"))?;

    let mut cfg = InstanceConfig::new(
        I::new_engine()?,
        "test_namespace".into(),
        "/containerd/address".into(),
    );
    let cfg = cfg
        .set_bundle(dir.path().to_str().unwrap().to_string())
        .set_stdout(dir.path().join("stdout").to_str().unwrap().to_string());

    let wasi = I::new("test".to_string(), Some(cfg));

    wasi.start()?;

    let (tx, rx) = channel();
    let waiter = Wait::new(tx);
    wasi.wait(&waiter).unwrap();

    let res = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(res) => Ok(res),
        Err(e) => {
            wasi.kill(SIGKILL as u32).unwrap();
            return Err(Error::Others(format!(
                "error waiting for module to finish: {0}",
                e
            )));
        }
    };
    wasi.delete()?;
    res
}
