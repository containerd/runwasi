use std::fs::read_to_string;
use std::os::unix::io::RawFd;
use std::os::unix::prelude::OsStrExt;

use anyhow::Result;
use containerd_shim_wasm::function;
use containerd_shim_wasm::sandbox::testutil::{
    has_cap_sys_admin, run_test_with_sudo, run_wasi_test,
};
use containerd_shim_wasm::sandbox::{Instance, InstanceConfig};
use libc::{dup2, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use serial_test::serial;
use tempfile::tempdir;
use wasmedge_sdk::wat2wasm;

use crate::WasmEdgeInstance;

//use super::*;

static mut STDIN_FD: Option<RawFd> = None;
static mut STDOUT_FD: Option<RawFd> = None;
static mut STDERR_FD: Option<RawFd> = None;

fn reset_stdio() {
    unsafe {
        if let Some(stdin) = STDIN_FD {
            let _ = dup2(stdin, STDIN_FILENO);
        }
        if let Some(stdout) = STDOUT_FD {
            let _ = dup2(stdout, STDOUT_FILENO);
        }
        if let Some(stderr) = STDERR_FD {
            let _ = dup2(stderr, STDERR_FILENO);
        }
    }
}

// This is taken from https://github.com/bytecodealliance/wasmtime/blob/6a60e8363f50b936e4c4fc958cb9742314ff09f3/docs/WASI-tutorial.md?plain=1#L270-L298
const WASI_HELLO_WAT: &[u8]= r#"(module
    ;; Import the required fd_write WASI function which will write the given io vectors to stdout
    ;; The function signature for fd_write is:
    ;; (File Descriptor, *iovs, iovs_len, nwritten) -> Returns number of bytes written
    (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))

    (memory 1)
    (export "memory" (memory 0))

    ;; Write 'hello world\n' to memory at an offset of 8 bytes
    ;; Note the trailing newline which is required for the text to appear
    (data (i32.const 8) "hello world\n")

    (func $main (export "_start")
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
"#.as_bytes();

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
fn test_delete_after_create() {
    let i = WasmEdgeInstance::new(
        "".to_string(),
        Some(&InstanceConfig::new(
            Default::default(),
            "test_namespace".into(),
            "/containerd/address".into(),
        )),
    );
    i.delete().unwrap();
}

#[test]
#[serial]
fn test_wasi() -> Result<()> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo(function!());
    }

    let dir = tempdir()?;
    let path = dir.path();
    let wasm_bytes = wat2wasm(WASI_HELLO_WAT).unwrap();

    let res = run_wasi_test::<WasmEdgeInstance>(&dir, wasm_bytes, None)?;

    assert_eq!(res.0, 0);

    let output = read_to_string(path.join("stdout"))?;
    assert_eq!(output, "hello world\n");

    reset_stdio();
    Ok(())
}

#[test]
#[serial]
fn test_wasi_error() -> Result<()> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo(function!());
    }

    let dir = tempdir()?;
    let wasm_bytes = wat2wasm(WASI_RETURN_ERROR).unwrap();

    let res = run_wasi_test::<WasmEdgeInstance>(&dir, wasm_bytes, None)?;

    // Expect error code from the run.
    assert_eq!(res.0, 137);

    reset_stdio();
    Ok(())
}

#[test]
#[serial]
fn test_wasi_exit_code() -> Result<()> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo(function!());
    }

    let expected_exit_code: u32 = 42;

    let dir = tempdir()?;
    let wasm_bytes = module_with_exit_code(expected_exit_code);
    let (actual_exit_code, _) = run_wasi_test::<WasmEdgeInstance>(&dir, wasm_bytes, None)?;

    assert_eq!(actual_exit_code, expected_exit_code);

    reset_stdio();
    Ok(())
}

// Get the path to binary where the `WasmEdge_VersionGet` C ffi symbol is defined.
// If wasmedge is dynamically linked, this will be the path to the `.so`.
// If wasmedge is statically linked, this will be the path to the current executable.
fn get_wasmedge_binary_path() -> Option<std::path::PathBuf> {
    let f = wasmedge_sys::ffi::WasmEdge_VersionGet;
    let mut info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(f as *const libc::c_void, &mut info) } == 0 {
        None
    } else {
        let fname = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
        let fname = std::ffi::OsStr::from_bytes(fname.to_bytes());
        Some(std::path::PathBuf::from(fname))
    }
}

#[cfg(feature = "static")]
#[test]
fn check_static_linking() {
    let wasmedge_path = get_wasmedge_binary_path().unwrap().canonicalize().unwrap();
    let current_exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert!(wasmedge_path == current_exe);
}

#[cfg(not(feature = "static"))]
#[test]
fn check_dynamic_linking() {
    let wasmedge_path = get_wasmedge_binary_path().unwrap().canonicalize().unwrap();
    let current_exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert!(wasmedge_path != current_exe);
}
