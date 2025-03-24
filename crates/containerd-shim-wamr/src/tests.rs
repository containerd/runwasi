use std::time::Duration;

use containerd_shim_wasm::testing::WasiTest;
use containerd_shim_wasm::testing::modules::*;
use serial_test::serial;

use crate::WamrShim as WasiEngine;

#[test]
#[serial]
fn test_delete_after_create() -> anyhow::Result<()> {
    WasiTest::<WasiEngine>::builder()?.build()?.delete()?;
    Ok(())
}

#[test]
#[serial]
fn test_hello_world() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiEngine>::builder()?
        .with_wasm(HELLO_WORLD)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[test]
#[serial]
fn test_hello_world_oci() -> anyhow::Result<()> {
    let (builder, _oci_cleanup) = WasiTest::<WasiEngine>::builder()?
        .with_wasm(HELLO_WORLD)?
        .as_oci_image(None, None)?;

    let (exit_code, stdout, _) = builder.build()?.start()?.wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}
#[test]
#[serial]
fn test_unreachable() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiEngine>::builder()?
        .with_wasm(UNREACHABLE)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_ne!(exit_code, 0);

    Ok(())
}

#[test]
#[serial]
fn test_seccomp() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiEngine>::builder()?
        .with_wasm(SECCOMP)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout.trim(), "current working dir: /");

    Ok(())
}

#[test]
#[serial]
fn test_has_default_devices() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiEngine>::builder()?
        .with_wasm(HAS_DEFAULT_DEVICES)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 0);

    Ok(())
}

#[test]
#[ignore = "disabled because the WAMR SDK doesn't expose exit code yet"]
// See https://github.com/containerd/runwasi/pull/716#discussion_r1827086060
fn test_exit_code() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiEngine>::builder()?
        .with_wasm(EXIT_CODE)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 42);

    Ok(())
}

#[test]
#[ignore]
// See https://github.com/containerd/runwasi/pull/716#issuecomment-2458200081
fn test_custom_entrypoint() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiEngine>::builder()?
        .with_start_fn("foo")
        .with_wasm(CUSTOM_ENTRYPOINT)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}
