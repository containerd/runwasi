use std::fs::File;
use std::io::prelude::*;
use std::io::ErrorKind;
use std::os::unix::io::RawFd;
use std::sync::{Arc, Condvar, Mutex};

use anyhow::Context;
use anyhow::Result;
use chrono::{DateTime, Utc};
use containerd_shim_wasm::sandbox::error::Error;
use containerd_shim_wasm::sandbox::instance::YoukiInstance;
use containerd_shim_wasm::sandbox::instance_utils::maybe_open_stdio;
use containerd_shim_wasm::sandbox::{EngineGetter, Instance, InstanceConfig};
use libc::{dup2, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use nix::unistd::close;
use serde::{Deserialize, Serialize};
use wasmedge_sdk::{
    config::{CommonConfigOptions, ConfigBuilder, HostRegistrationConfigOptions},
    plugin::PluginManager,
    Vm, VmBuilder,
};

use std::{
    fs,
    path::{Path, PathBuf},
};

use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::Container;
use libcontainer::syscall::syscall::create_syscall;

use crate::executor::WasmEdgeExecutor;

static mut STDIN_FD: Option<RawFd> = None;
static mut STDOUT_FD: Option<RawFd> = None;
static mut STDERR_FD: Option<RawFd> = None;

static DEFAULT_CONTAINER_ROOT_DIR: &str = "/run/containerd/wasmedge";

type ExitCode = Arc<(Mutex<Option<(u32, DateTime<Utc>)>>, Condvar)>;
pub struct Wasi {
    id: String,

    exit_code: ExitCode,

    stdin: String,
    stdout: String,
    stderr: String,
    bundle: String,

    rootdir: PathBuf,
}

pub fn reset_stdio() {
    unsafe {
        if STDIN_FD.is_some() {
            dup2(STDIN_FD.unwrap(), STDIN_FILENO);
        }
        if STDOUT_FD.is_some() {
            dup2(STDOUT_FD.unwrap(), STDOUT_FILENO);
        }
        if STDERR_FD.is_some() {
            dup2(STDERR_FD.unwrap(), STDERR_FILENO);
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

fn determine_rootdir<P: AsRef<Path>>(bundle: P, namespace: String) -> Result<PathBuf, Error> {
    let mut file = match File::open(bundle.as_ref().join("options.json")) {
        Ok(f) => f,
        Err(err) => match err.kind() {
            ErrorKind::NotFound => {
                return Ok(<&str as Into<PathBuf>>::into(DEFAULT_CONTAINER_ROOT_DIR).join(namespace))
            }
            _ => return Err(err.into()),
        },
    };
    let mut data = String::new();
    file.read_to_string(&mut data)?;
    let options: Options = serde_json::from_str(&data)?;
    Ok(options
        .root
        .unwrap_or(PathBuf::from(DEFAULT_CONTAINER_ROOT_DIR))
        .join(namespace))
}

impl YoukiInstance for Wasi {
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
        let stdin = maybe_open_stdio(self.stdin.as_str()).context("could not open stdin")?;
        let stdout = maybe_open_stdio(self.stdout.as_str()).context("could not open stdout")?;
        let stderr = maybe_open_stdio(self.stderr.as_str()).context("could not open stderr")?;

        let syscall = create_syscall();
        let err_msg = |err| format!("failed to create container: {}", err);
        let container = ContainerBuilder::new(self.id.clone(), syscall.as_ref())
            .with_executor(vec![Box::new(WasmEdgeExecutor {
                stdin,
                stdout,
                stderr,
            })])
            .map_err(|err| Error::Others(err_msg(err)))?
            .with_root_path(self.rootdir.clone())
            .map_err(|err| Error::Others(err_msg(err)))?
            .as_init(&self.bundle)
            .with_systemd(false)
            .build()
            .map_err(|err| Error::Others(err_msg(err)))?;
        // Close the fds now that they have been passed to the container process
        // so that we don't leak them.
        stdin.map(close);
        stdout.map(close);
        stderr.map(close);

        Ok(container)
    }
}

impl Instance for Wasi {
    type E = Vm;
    fn new(id: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self {
        let cfg = cfg.unwrap(); // TODO: handle error
        let bundle = cfg.get_bundle().unwrap_or_default();
        let namespace = cfg.get_namespace();
        Wasi {
            id,
            rootdir: determine_rootdir(bundle.as_str(), namespace).unwrap(),
            exit_code: Arc::new((Mutex::new(None), Condvar::new())),
            stdin: cfg.get_stdin().unwrap_or_default(),
            stdout: cfg.get_stdout().unwrap_or_default(),
            stderr: cfg.get_stderr().unwrap_or_default(),
            bundle,
        }
    }

    fn start(&self) -> std::result::Result<u32, Error> {
        self.start_youki()
    }

    fn kill(&self, signal: u32) -> std::result::Result<(), Error> {
        self.kill_youki(signal)
    }

    fn delete(&self) -> std::result::Result<(), Error> {
        self.delete_youki()
    }

    fn wait(
        &self,
        waiter: &containerd_shim_wasm::sandbox::instance::Wait,
    ) -> std::result::Result<(), Error> {
        self.wait_youki(waiter)
    }
}

impl EngineGetter for Wasi {
    type E = Vm;
    fn new_engine() -> Result<Vm, Error> {
        PluginManager::load(None).unwrap();
        let mut host_options = HostRegistrationConfigOptions::default();
        host_options = host_options.wasi(true);
        #[cfg(all(target_os = "linux", feature = "wasi_nn", target_arch = "x86_64"))]
        {
            host_options = host_options.wasi_nn(true);
        }
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .with_host_registration_config(host_options)
            .build()
            .map_err(anyhow::Error::msg)?;
        let vm = VmBuilder::new()
            .with_config(config)
            .build()
            .map_err(anyhow::Error::msg)?;

        Ok(vm)
    }
}

#[cfg(test)]
mod wasitest {
    use std::borrow::Cow;
    use std::fs::{create_dir, read_to_string, File, OpenOptions};
    use std::os::unix::prelude::OpenOptionsExt;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use containerd_shim_wasm::function;
    use containerd_shim_wasm::sandbox::instance::Wait;
    use containerd_shim_wasm::sandbox::testutil::{has_cap_sys_admin, run_test_with_sudo};
    use libc::SIGKILL;
    use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
    use tempfile::{tempdir, TempDir};

    use serial_test::serial;

    use super::*;

    use wasmedge_sdk::{
        config::{CommonConfigOptions, ConfigBuilder},
        wat2wasm,
    };

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

    fn run_wasi_test(dir: &TempDir, wasmbytes: Cow<[u8]>) -> Result<(u32, DateTime<Utc>), Error> {
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

        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec!["./hello.wasm".to_string()])
                    .build()?,
            )
            .build()?;

        spec.save(dir.path().join("config.json"))?;

        let mut cfg = InstanceConfig::new(
            Wasi::new_engine()?,
            "test_namespace".into(),
            "/containerd/address".into(),
        );
        let cfg = cfg
            .set_bundle(dir.path().to_str().unwrap().to_string())
            .set_stdout(dir.path().join("stdout").to_str().unwrap().to_string());

        let wasi = Wasi::new("test".to_string(), Some(cfg));

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

    #[test]
    #[serial]
    fn test_delete_after_create() {
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .build()
            .unwrap();
        let vm = VmBuilder::new().with_config(config).build().unwrap();
        let i = Wasi::new(
            "".to_string(),
            Some(&InstanceConfig::new(
                vm,
                "test_namespace".into(),
                "/containerd/address".into(),
            )),
        );
        i.delete().unwrap();
    }

    #[test]
    #[serial]
    fn test_wasi() -> Result<(), Error> {
        if !has_cap_sys_admin() {
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }

        let dir = tempdir()?;
        let path = dir.path();
        let wasmbytes = wat2wasm(WASI_HELLO_WAT).unwrap();

        let res = run_wasi_test(&dir, wasmbytes)?;

        assert_eq!(res.0, 0);

        let output = read_to_string(path.join("stdout"))?;
        assert_eq!(output, "hello world\n");

        reset_stdio();
        Ok(())
    }

    #[test]
    #[serial]
    fn test_wasi_error() -> Result<(), Error> {
        if !has_cap_sys_admin() {
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }

        let dir = tempdir()?;
        let wasmbytes = wat2wasm(WASI_RETURN_ERROR).unwrap();

        let res = run_wasi_test(&dir, wasmbytes)?;

        // Expect error code from the run.
        assert_eq!(res.0, 137);

        reset_stdio();
        Ok(())
    }
}

#[cfg(test)]
mod rootdirtest {
    use std::fs::OpenOptions;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_determine_rootdir_with_options_file() -> Result<(), Error> {
        let namespace = "test_namespace";
        let dir = tempdir()?;
        let rootdir = dir.path().join("runwasi");
        let opts = Options {
            root: Some(rootdir.clone()),
        };
        let opts_file = OpenOptions::new()
            .read(true)
            .create(true)
            .truncate(true)
            .write(true)
            .open(dir.path().join("options.json"))?;
        write!(&opts_file, "{}", serde_json::to_string(&opts)?)?;
        let root = determine_rootdir(dir.path(), namespace.into())?;
        assert_eq!(root, rootdir.join(namespace));
        Ok(())
    }

    #[test]
    fn test_determine_rootdir_without_options_file() -> Result<(), Error> {
        let dir = tempdir()?;
        let namespace = "test_namespace";
        let root = determine_rootdir(dir.path(), namespace.into())?;
        assert!(root.is_absolute());
        assert_eq!(
            root,
            PathBuf::from(DEFAULT_CONTAINER_ROOT_DIR).join(namespace)
        );
        Ok(())
    }
}
