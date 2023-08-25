//! Testing utilities used across different modules

use std::collections::HashMap;
use std::fs::{create_dir, File};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::{bail, ensure, Result};

use crate::sys::signals::SIGKILL;

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

    ensure!(cmd.wait()?.success(), "running test with sudo failed");

    Ok(())
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
use chrono::{DateTime, Utc};
pub use function;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};

use super::instance::Wait;
use super::{Instance, InstanceConfig};

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

pub fn run_wasi_test<WasmInstance: Instance>(
    dir: impl AsRef<Path>,
    wasmbytes: impl AsRef<[u8]>,
    start_fn: Option<&str>,
) -> Result<(u32, DateTime<Utc>)>
where
    WasmInstance::Engine: Default,
{
    create_dir(dir.as_ref().join("rootfs"))?;
    let rootdir = dir.as_ref().join("runwasi");
    create_dir(&rootdir)?;
    let opts = HashMap::from([("root", rootdir)]);
    let opts_file = std::fs::File::create(dir.as_ref().join("options.json"))?;
    serde_json::to_writer(opts_file, &opts)?;

    let filename = if wasmbytes.as_ref().starts_with(b"\0asm") {
        "hello.wasm"
    } else {
        "hello.wat"
    };

    let wasm_path = dir.as_ref().join("rootfs").join(filename);
    std::fs::write(&wasm_path, wasmbytes)?;

    #[cfg(unix)]
    std::fs::set_permissions(
        &wasm_path,
        std::os::unix::prelude::PermissionsExt::from_mode(0o755),
    )?;

    let stdout = File::create(dir.as_ref().join("stdout"))?;
    drop(stdout);

    let entrypoint = match start_fn {
        Some(s) => format!("./{filename}#{s}"),
        None => format!("./{filename}"),
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

    spec.save(dir.as_ref().join("config.json"))?;

    let mut cfg = InstanceConfig::new(
        WasmInstance::Engine::default(),
        "test_namespace".into(),
        "/containerd/address".into(),
    );
    let cfg = cfg
        .set_bundle(dir.as_ref().to_str().unwrap().to_string())
        .set_stdout(dir.as_ref().join("stdout").to_str().unwrap().to_string());

    let wasi = WasmInstance::new("test".to_string(), Some(cfg));

    wasi.start()?;

    let (tx, rx) = channel();
    let waiter = Wait::new(tx);
    wasi.wait(&waiter).unwrap();

    let res = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(res) => Ok(res),
        Err(e) => {
            wasi.kill(SIGKILL as u32).unwrap();
            bail!("error waiting for module to finish: {e}");
        }
    };
    wasi.delete()?;
    res
}
