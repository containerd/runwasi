use std::time::Duration;

//use containerd_shim_wasm::sandbox::Instance;
use containerd_shim_wasm::testing::modules::*;
use containerd_shim_wasm::testing::WasiTest;
use serial_test::serial;

use crate::instance::WasmerInstance as WasiInstance;

#[test]
#[serial]
fn test_delete_after_create() -> anyhow::Result<()> {
    WasiTest::<WasiInstance>::builder()?.build()?.delete()?;
    Ok(())
}

#[test]
#[serial]
fn test_hello_world() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
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
    let (builder, _oci_cleanup) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .as_oci_image(None, None)?;

    let (exit_code, stdout, _) = builder.build()?.start()?.wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[test]
#[serial]
fn test_custom_entrypoint() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
        .with_start_fn("foo")
        .with_wasm(CUSTOM_ENTRYPOINT)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[test]
#[serial]
fn test_unreachable() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(UNREACHABLE)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_ne!(exit_code, 0);

    Ok(())
}

#[test]
#[serial]
fn test_exit_code() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(EXIT_CODE)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 42);

    Ok(())
}

#[test]
#[serial]
fn test_seccomp() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
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
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HAS_DEFAULT_DEVICES)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;

    assert_eq!(exit_code, 0);

    Ok(())
}
