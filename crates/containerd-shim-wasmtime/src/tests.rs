use std::fs::read_to_string;

use anyhow::Result;
use containerd_shim_wasm::function;
use containerd_shim_wasm::sandbox::testutil::{
    has_cap_sys_admin, run_test_with_sudo, run_wasi_test,
};
use containerd_shim_wasm::sandbox::{Instance as SandboxInstance, InstanceConfig, Stdio};
use serial_test::serial;
use tempfile::tempdir;

use crate::WasmtimeInstance as Instance;

// This is taken from https://github.com/bytecodealliance/wasmtime/blob/6a60e8363f50b936e4c4fc958cb9742314ff09f3/docs/WASI-tutorial.md?plain=1#L270-L298
fn hello_world_module(start_fn: Option<&str>) -> Vec<u8> {
    let start_fn = start_fn.unwrap_or("_start");
    format!(r#"(module
        ;; Import the required fd_write WASI function which will write the given io vectors to stdout
        ;; The function signature for fd_write is:
        ;; (File Descriptor, *iovs, iovs_len, nwritten) -> Returns number of bytes written
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))

        (memory 1)
        (export "memory" (memory 0))

        ;; Write 'hello world\n' to memory at an offset of 8 bytes
        ;; Note the trailing newline which is required for the text to appear
        (data (i32.const 8) "hello world\n")

        (func $main (export "{start_fn}")
            ;; Creating a new io vector within linear memory
            (i32.store (i32.const 0) (i32.const 8))  ;; iov.iov_base - This is a pointer to the start of the 'hello world\n' string
            (i32.store (i32.const 4) (i32.const 12))  ;; iov.iov_len - The length of the 'hello world\n' string

            (call $fd_write
                (i32.const 1) ;; file_descriptor - 1 for stdout
                (i32.const 0) ;; *iovs - The pointer to the iov array, which is stored at memory location 0
                (i32.const 1) ;; iovs_len - We're printing 1 string stored in an iov - so one.
                (i32.const 20) ;; nwritten - A place in memory to store the number of bytes written
            )
            drop ;; Discard the number of bytes written from the top of the stack
        )
    )
    "#).as_bytes().to_vec()
}

fn module_with_exit_code(exit_code: u32) -> Vec<u8> {
    format!(r#"(module
        ;; Import the required proc_exit WASI function which terminates the program with an exit code.
        ;; The function signature for proc_exit is:
        ;; (exit_code: i32) -> !
        (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
        (memory 1)
        (export "memory" (memory 0))
        (func $main (export "_start")
            (call $proc_exit (i32.const {exit_code}))
            unreachable
        )
    )
    "#).as_bytes().to_vec()
}

const WASI_RETURN_ERROR: &[u8] = r#"(module
    (func $main (export "_start")
        (unreachable)
    )
)
"#
.as_bytes();

#[test]
#[serial]
fn test_delete_after_create() -> Result<()> {
    // start logging
    let _ = env_logger::try_init();
    let _guard = Stdio::init_from_std().guard();

    let cfg = InstanceConfig::new(
        Default::default(),
        "test_namespace".into(),
        "/containerd/address".into(),
    );

    let i = Instance::new("".to_string(), Some(&cfg));
    i.delete()?;

    Ok(())
}

#[test]
#[serial]
fn test_wasi_entrypoint() -> Result<()> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo(function!());
    }

    // start logging
    // to enable logging run `export RUST_LOG=trace` and append cargo command with
    // --show-output before running test
    let _ = env_logger::try_init();
    let _guard = Stdio::init_from_std().guard();

    let dir = tempdir()?;
    let path = dir.path();
    let wasm_bytes = hello_world_module(None);

    let res = run_wasi_test::<Instance>(&dir, wasm_bytes, None)?;

    assert_eq!(res.0, 0);

    let output = read_to_string(path.join("stdout"))?;
    assert_eq!(output, "hello world\n");

    Ok(())
}

#[test]
#[serial]
fn test_wasi_custom_entrypoint() -> Result<()> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo(function!());
    }

    // start logging
    let _ = env_logger::try_init();
    let _guard = Stdio::init_from_std().guard();

    let dir = tempdir()?;
    let path = dir.path();
    let wasm_bytes = hello_world_module(Some("foo"));

    let res = run_wasi_test::<Instance>(&dir, wasm_bytes, Some("foo"))?;

    assert_eq!(res.0, 0);

    let output = read_to_string(path.join("stdout"))?;
    assert_eq!(output, "hello world\n");

    Ok(())
}

#[test]
#[serial]
fn test_wasi_error() -> Result<()> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo(function!());
    }

    // start logging
    let _ = env_logger::try_init();
    let _guard = Stdio::init_from_std().guard();

    let dir = tempdir()?;
    let res = run_wasi_test::<Instance>(&dir, WASI_RETURN_ERROR, None)?;

    // Expect error code from the run.
    assert_eq!(res.0, 137);

    Ok(())
}

#[test]
#[serial]
fn test_wasi_exit_code() -> Result<()> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo(function!());
    }

    // start logging
    let _ = env_logger::try_init();
    let _guard = Stdio::init_from_std().guard();

    let expected_exit_code: u32 = 42;

    let dir = tempdir()?;
    let wasm_bytes = module_with_exit_code(expected_exit_code);
    let (actual_exit_code, _) = run_wasi_test::<Instance>(&dir, wasm_bytes, None)?;

    assert_eq!(actual_exit_code, expected_exit_code);

    Ok(())
}
