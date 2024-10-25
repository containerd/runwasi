use std::time::Duration;

//use containerd_shim_wasm::sandbox::Instance;
use containerd_shim_wasm::testing::modules::*;
use containerd_shim_wasm::testing::WasiTest;
use serial_test::serial;

use crate::instance::WasmEdgeInstance as WasiInstance;

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

// Get the path to binary where the `WasmEdge_VersionGet` C ffi symbol is defined.
// If wasmedge is dynamically linked, this will be the path to the `.so`.
// If wasmedge is statically linked, this will be the path to the current executable.
fn get_wasmedge_binary_path() -> std::path::PathBuf {
    use std::os::unix::prelude::OsStrExt;

    extern "C" {
        pub fn WasmEdge_VersionGet() -> *const libc::c_char;
    }

    let mut info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(WasmEdge_VersionGet as *const libc::c_void, &mut info) } == 0 {
        // no dladdr support, must be a static binary
        std::env::current_exe().unwrap_or_default()
    } else {
        let fname = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
        let fname = std::ffi::OsStr::from_bytes(fname.to_bytes());
        std::path::PathBuf::from(fname)
    }
}

#[cfg(feature = "static")]
#[test]
fn check_static_linking() {
    let wasmedge_path = get_wasmedge_binary_path().canonicalize().unwrap();
    let current_exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert!(wasmedge_path == current_exe);
}

#[cfg(not(feature = "static"))]
#[test]
fn check_dynamic_linking() {
    let wasmedge_path = get_wasmedge_binary_path().canonicalize().unwrap();
    let current_exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert!(wasmedge_path != current_exe);
}
