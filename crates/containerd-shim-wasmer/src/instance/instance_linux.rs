use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use anyhow::Result;
use containerd_shim_wasm::libcontainer_instance::LibcontainerInstance;
use containerd_shim_wasm::sandbox::instance::ExitCode;
use containerd_shim_wasm::sandbox::instance_utils::determine_rootdir;
use containerd_shim_wasm::sandbox::{Error, InstanceConfig, Stdio};
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::Container;
use libcontainer::syscall::syscall::SyscallType;
use serde::{Deserialize, Serialize};

use crate::executor::WasmerExecutor;

static DEFAULT_CONTAINER_ROOT_DIR: &str = "/run/containerd/wasmer";

pub struct Wasi {
    exit_code: ExitCode,
    engine: wasmer::Cranelift,
    stdio: Stdio,
    bundle: String,
    rootdir: PathBuf,
    id: String,
}

impl LibcontainerInstance for Wasi {
    type Engine = wasmer::Cranelift;

    fn new_libcontainer(id: String, cfg: Option<&InstanceConfig<Self::Engine>>) -> Self {
        // TODO: there are failure cases e.x. parsing cfg, loading spec, etc.
        // thus should make `new` return `Result<Self, Error>` instead of `Self`
        log::info!("creating new instance: {}", id);
        let cfg = cfg.unwrap();
        let bundle = cfg.get_bundle().unwrap_or_default();
        let rootdir = determine_rootdir(
            bundle.as_str(),
            &cfg.get_namespace(),
            DEFAULT_CONTAINER_ROOT_DIR,
        )
        .unwrap();
        Wasi {
            id,
            exit_code: Arc::new((Mutex::new(None), Condvar::new())),
            engine: cfg.get_engine(),
            stdio: Stdio::init_from_cfg(cfg).expect("failed to open stdio"),
            bundle,
            rootdir,
        }
    }

    fn build_container(&self) -> Result<Container, Error> {
        let engine = self.engine.clone();
        let err_others = |err| Error::Others(format!("failed to create container: {}", err));

        let container = ContainerBuilder::new(self.id.clone(), SyscallType::Linux)
            .with_executor(WasmerExecutor::new(self.stdio.take().clone(), engine))
            .with_root_path(self.rootdir.clone())
            .map_err(err_others)?
            .as_init(&self.bundle)
            .with_systemd(false)
            .build()
            .map_err(err_others)?;

        Ok(container)
    }

    fn get_exit_code(&self) -> ExitCode {
        self.exit_code.clone()
    }

    fn get_id(&self) -> String {
        self.id.clone()
    }

    fn get_root_dir(&self) -> Result<PathBuf, Error> {
        Ok(self.rootdir.clone())
    }
}

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

#[cfg(test)]
mod wasitest {

    use std::fs::read_to_string;
    use std::os::fd::RawFd;

    use containerd_shim_wasm::function;
    use containerd_shim_wasm::sandbox::testutil::{
        has_cap_sys_admin, run_test_with_sudo, run_wasi_test,
    };
    use containerd_shim_wasm::sandbox::Instance;
    use libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
    use nix::unistd::dup2;
    use serial_test::serial;
    use tempfile::tempdir;

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

    #[test]
    #[serial]
    fn test_delete_after_create() -> anyhow::Result<()> {
        let cfg = InstanceConfig::new(
            Default::default(),
            "test_namespace".into(),
            "/containerd/address".into(),
        );

        let i = Wasi::new("".to_string(), Some(&cfg));
        i.delete()?;
        reset_stdio();
        Ok(())
    }

    #[test]
    #[serial]
    fn test_wasi_entrypoint() -> anyhow::Result<()> {
        if !has_cap_sys_admin() {
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }
        // start logging
        // to enable logging run `export RUST_LOG=trace` and append cargo command with
        // --show-output before running test
        let _ = env_logger::try_init();

        let dir = tempdir()?;
        let path = dir.path();
        let wasm_bytes = hello_world_module(None);

        let res = run_wasi_test::<Wasi>(&dir, wasm_bytes, None)?;

        assert_eq!(res.0, 0);

        let output = read_to_string(path.join("stdout"))?;
        assert_eq!(output, "hello world\n");

        reset_stdio();
        Ok(())
    }

    // ignore until https://github.com/containerd/runwasi/issues/194 is resolved
    #[test]
    #[serial]
    fn test_wasi_custom_entrypoint() -> anyhow::Result<()> {
        if !has_cap_sys_admin() {
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }
        // start logging
        let _ = env_logger::try_init();

        let dir = tempdir()?;
        let path = dir.path();
        let wasm_bytes = hello_world_module(Some("foo"));

        let res = run_wasi_test::<Wasi>(&dir, wasm_bytes, Some("foo"))?;

        assert_eq!(res.0, 0);

        let output = read_to_string(path.join("stdout"))?;
        assert_eq!(output, "hello world\n");

        reset_stdio();
        Ok(())
    }

    #[test]
    #[serial]
    fn test_wasi_exit_code() -> anyhow::Result<()> {
        if !has_cap_sys_admin() {
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }

        // start logging
        let _ = env_logger::try_init();

        let expected_exit_code: u32 = 42;

        let dir = tempdir()?;
        let wasm_bytes = module_with_exit_code(expected_exit_code);
        log::info!("{:?}", wasm_bytes);
        let (actual_exit_code, _) = run_wasi_test::<Wasi>(&dir, wasm_bytes, None)?;

        assert_eq!(actual_exit_code, expected_exit_code);

        reset_stdio();
        Ok(())
    }
}
