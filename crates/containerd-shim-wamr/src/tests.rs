use std::time::Duration;

use containerd_shim_wasm::testing::WasiTest;
//use containerd_shim_wasm::sandbox::Instance;
use containerd_shim_wasm::testing::modules::*;
use serial_test::serial;

use crate::instance::WamrInstance as WasiInstance;

#[tokio::test]
#[serial]
async fn test_delete_after_create() -> anyhow::Result<()> {
    WasiTest::<WasiInstance>::builder()?
        .build()
        .await?
        .delete()
        .await?;
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_hello_world() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_hello_world_oci() -> anyhow::Result<()> {
    let (builder, _oci_cleanup) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .as_oci_image(None, None)?;

    let (exit_code, stdout, _) = builder
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}
#[tokio::test]
#[serial]
async fn test_unreachable() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(UNREACHABLE)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_ne!(exit_code, 0);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_seccomp() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(SECCOMP)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout.trim(), "current working dir: /");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_has_default_devices() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HAS_DEFAULT_DEVICES)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);

    Ok(())
}

#[tokio::test]
#[ignore = "disabled because the WAMR SDK doesn't expose exit code yet"]
// See https://github.com/containerd/runwasi/pull/716#discussion_r1827086060
async fn test_exit_code() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(EXIT_CODE)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 42);

    Ok(())
}

#[tokio::test]
#[ignore]
// See https://github.com/containerd/runwasi/pull/716#issuecomment-2458200081
async fn test_custom_entrypoint() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
        .with_start_fn("foo")
        .with_wasm(CUSTOM_ENTRYPOINT)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}
