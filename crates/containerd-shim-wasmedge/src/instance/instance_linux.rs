use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use containerd_shim_wasm::libcontainer_instance::{LibcontainerInstance, LinuxContainerExecutor};
use containerd_shim_wasm::sandbox::error::Error;
use containerd_shim_wasm::sandbox::instance::ExitCode;
use containerd_shim_wasm::sandbox::instance_utils::determine_rootdir;
use containerd_shim_wasm::sandbox::{InstanceConfig, Stdio};
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::Container;
use libcontainer::syscall::syscall::create_syscall;

use crate::executor::WasmEdgeExecutor;

static DEFAULT_CONTAINER_ROOT_DIR: &str = "/run/containerd/wasmedge";

pub struct Wasi {
    id: String,
    exit_code: ExitCode,
    stdio: Stdio,
    bundle: String,
    rootdir: PathBuf,
}

impl LibcontainerInstance for Wasi {
    type Engine = ();

    fn new_libcontainer(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Self {
        let cfg = cfg.unwrap(); // TODO: handle error
        let bundle = cfg.get_bundle().unwrap_or_default();
        let namespace = cfg.get_namespace();
        Wasi {
            id,
            rootdir: determine_rootdir(
                bundle.as_str(),
                namespace.as_str(),
                DEFAULT_CONTAINER_ROOT_DIR,
            )
            .unwrap(),
            exit_code: Arc::new((Mutex::new(None), Condvar::new())),
            stdio: Stdio {
                stdin: cfg.get_stdin().try_into().unwrap(),
                stdout: cfg.get_stdout().try_into().unwrap(),
                stderr: cfg.get_stderr().try_into().unwrap(),
            },
            bundle,
        }
    }

    fn get_exit_code(&self) -> ExitCode {
        self.exit_code.clone()
    }

    fn get_id(&self) -> String {
        self.id.clone()
    }

    fn get_root_dir(&self) -> std::result::Result<PathBuf, Error> {
        Ok(self.rootdir.clone())
    }

    fn build_container(&self) -> std::result::Result<Container, Error> {
        fs::create_dir_all(&self.rootdir)?;

        let syscall = create_syscall();
        let err_others = |err| Error::Others(format!("failed to create container: {}", err));
        let default_executor = Box::new(LinuxContainerExecutor::new(self.stdio.clone()));
        let wasmedge_executor = Box::new(WasmEdgeExecutor::new(self.stdio.clone()));

        let container = ContainerBuilder::new(self.id.clone(), syscall.as_ref())
            .with_executor(vec![default_executor, wasmedge_executor])
            .map_err(err_others)?
            .with_root_path(self.rootdir.clone())
            .map_err(err_others)?
            .as_init(&self.bundle)
            .with_systemd(false)
            .build()
            .map_err(err_others)?;

        Ok(container)
    }
}

#[cfg(test)]
mod wasitest {

    use std::fs::read_to_string;
    use std::os::unix::io::RawFd;

    use containerd_shim_wasm::function;
    use containerd_shim_wasm::sandbox::testutil::{
        has_cap_sys_admin, run_test_with_sudo, run_wasi_test,
    };
    use containerd_shim_wasm::sandbox::Instance;
    use libc::{dup2, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
    use serial_test::serial;
    use tempfile::tempdir;
    use wasmedge_sdk::wat2wasm;

    use super::*;

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
        let i = Wasi::new(
            "".to_string(),
            Some(&InstanceConfig::new(
                (),
                "test_namespace".into(),
                "/containerd/address".into(),
            )),
        );
        i.delete().unwrap();
    }

    #[test]
    #[serial]
    fn test_wasi() -> anyhow::Result<()> {
        if !has_cap_sys_admin() {
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }

        let dir = tempdir()?;
        let path = dir.path();
        let wasm_bytes = wat2wasm(WASI_HELLO_WAT).unwrap();

        let res = run_wasi_test::<Wasi>(&dir, wasm_bytes, None)?;

        assert_eq!(res.0, 0);

        let output = read_to_string(path.join("stdout"))?;
        assert_eq!(output, "hello world\n");

        reset_stdio();
        Ok(())
    }

    #[test]
    #[serial]
    fn test_wasi_error() -> anyhow::Result<()> {
        if !has_cap_sys_admin() {
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }

        let dir = tempdir()?;
        let wasm_bytes = wat2wasm(WASI_RETURN_ERROR).unwrap();

        let res = run_wasi_test::<Wasi>(&dir, wasm_bytes, None)?;

        // Expect error code from the run.
        assert_eq!(res.0, 137);

        reset_stdio();
        Ok(())
    }
}
