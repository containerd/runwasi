use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use containerd_shim_wasm::sandbox::error::Error;
use containerd_shim_wasm::sandbox::exec;
use containerd_shim_wasm::sandbox::oci;
use containerd_shim_wasm::sandbox::{EngineGetter, Instance, InstanceConfig};
use libc::{dup2, SIGINT, SIGKILL, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use log::{debug, error};
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::os::unix::io::{IntoRawFd, RawFd};
use std::sync::{
    mpsc::Sender,
    {Arc, Condvar, Mutex},
};
use std::thread;
use std::time::Duration;

use wasmedge_sdk::{
    config::{CommonConfigOptions, ConfigBuilder, HostRegistrationConfigOptions},
    PluginManager, Vm,
};

use std::{
    fs,
    path::{Path, PathBuf},
};

use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::{Container, ContainerStatus};
use libcontainer::signal::Signal;
use libcontainer::syscall::syscall::create_syscall;

use crate::executor::WasmEdgeExecutor;

static mut STDIN_FD: Option<RawFd> = None;
static mut STDOUT_FD: Option<RawFd> = None;
static mut STDERR_FD: Option<RawFd> = None;

static ROOT_DIR: &str = "/var/run/runwasi";

type ExitCode = (Mutex<Option<(u32, DateTime<Utc>)>>, Condvar);
pub struct Wasi {
    id: String,

    exit_code: Arc<ExitCode>,
    engine: Vm,

    stdin: String,
    stdout: String,
    stderr: String,
    bundle: String,

    pidfd: Arc<Mutex<Option<exec::PidFD>>>,
}

fn construct_container_root<P: AsRef<Path>>(root_path: P, container_id: &str) -> Result<PathBuf> {
    // resolves relative paths, symbolic links etc. and get complete path
    let root_path = fs::canonicalize(&root_path).with_context(|| {
        format!(
            "failed to canonicalize {} for container {}",
            root_path.as_ref().display(),
            container_id
        )
    })?;
    // the state of the container is stored in a directory named after the container id
    Ok(root_path.join(container_id))
}

fn load_container<P: AsRef<Path>>(root_path: P, container_id: &str) -> Result<Container> {
    let container_root = construct_container_root(root_path, container_id)?;
    if !container_root.exists() {
        bail!("container {} does not exist.", container_id)
    }

    Container::load(container_root)
        .with_context(|| format!("could not load state for container {container_id}"))
}

fn container_exists<P: AsRef<Path>>(root_path: P, container_id: &str) -> Result<bool> {
    let container_root = construct_container_root(root_path, container_id)?;
    Ok(container_root.exists())
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_maybe_open_stdio() -> Result<(), Error> {
        let f = maybe_open_stdio("")?;
        assert!(f.is_none());

        let f = maybe_open_stdio("/some/nonexistent/path")?;
        assert!(f.is_none());

        let dir = tempdir()?;
        let temp = File::create(dir.path().join("testfile"))?;
        drop(temp);
        let f = maybe_open_stdio(dir.path().join("testfile").as_path().to_str().unwrap())?;
        assert!(f.is_some());
        Ok(())
    }
}

/// containerd can send an empty path or a non-existant path
/// In both these cases we should just assume that the stdio stream was not setup (intentionally)
/// Any other error is a real error.
fn maybe_open_stdio(path: &str) -> Result<Option<RawFd>, Error> {
    if path.is_empty() {
        return Ok(None);
    }
    match OpenOptions::new().read(true).write(true).open(path) {
        Ok(f) => Ok(Some(f.into_raw_fd())),
        Err(err) => match err.kind() {
            ErrorKind::NotFound => Ok(None),
            _ => Err(err.into()),
        },
    }
}

fn load_spec(bundle: String) -> Result<oci::Spec, Error> {
    let mut spec = oci::load(Path::new(&bundle).join("config.json").to_str().unwrap())?;
    spec.canonicalize_rootfs(&bundle)
        .map_err(|e| Error::Others(format!("error canonicalizing rootfs in spec: {}", e)))?;
    Ok(spec)
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

impl Instance for Wasi {
    type E = Vm;
    fn new(id: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self {
        let cfg = cfg.unwrap(); // TODO: handle error
        Wasi {
            id,
            exit_code: Arc::new((Mutex::new(None), Condvar::new())),
            engine: cfg.get_engine(),
            stdin: cfg.get_stdin().unwrap_or_default(),
            stdout: cfg.get_stdout().unwrap_or_default(),
            stderr: cfg.get_stderr().unwrap_or_default(),
            bundle: cfg.get_bundle().unwrap_or_default(),
            pidfd: Arc::new(Mutex::new(None)),
        }
    }

    fn start(&self) -> Result<u32, Error> {
        debug!("preparing module");
        let syscall = create_syscall();
        let mut container = ContainerBuilder::new(
            self.id.clone(),
            syscall.as_ref(),
            vec![Box::new(WasmEdgeExecutor {
                stdin: maybe_open_stdio(self.stdin.as_str()).context("could not open stdin")?,
                stdout: maybe_open_stdio(self.stdout.as_str()).context("could not open stdout")?,
                stderr: maybe_open_stdio(self.stderr.as_str()).context("could not open stderr")?,
            })],
        )
        .with_root_path(ROOT_DIR)?
        .as_init(&self.bundle)
        .with_systemd(false)
        .build()?;

        let code = self.exit_code.clone();
        let id = self.id.clone();
        thread::spawn(move || {
            let (lock, cvar) = &*code;
            // FIX: https://github.com/containers/youki/issues/1601
            // let status = match waitpid(pid, None).unwrap() {
            //     WaitStatus::Exited(_, status) => status,
            //     WaitStatus::Signaled(_, sig, _) => sig as i32,
            //     _ => 0,
            // };
            thread::sleep(Duration::from_millis(100));
            let mut container = load_container(ROOT_DIR, id.as_str()).unwrap();
            let status: u32;
            loop {
                match container.status() {
                    ContainerStatus::Stopped => {
                        status = 0;
                        break;
                    }
                    _ => {
                        thread::sleep(Duration::from_millis(100));
                        container.refresh_status().unwrap();
                        continue;
                    }
                }
            }

            let mut ec = lock.lock().unwrap();
            *ec = Some((status, Utc::now()));
            drop(ec);
            cvar.notify_all();
        });

        container.start()?;

        Ok(0)
    }

    fn kill(&self, signal: u32) -> Result<(), Error> {
        if signal as i32 != SIGKILL && signal as i32 != SIGINT {
            println!("{:?}", signal);
            return Err(Error::InvalidArgument(
                "only SIGKILL and SIGINT are supported".to_string(),
            ));
        }

        let mut container = load_container(ROOT_DIR, self.id.as_str())?;
        match container.kill(Signal::try_from(signal as i32)?, true) {
            Ok(_) => Ok(()),
            Err(e) => {
                if container.status() == ContainerStatus::Stopped {
                    return Err(Error::Others("container not running".into()));
                }
                Err(Error::Others(e.to_string()))
            }
        }
    }

    fn delete(&self) -> Result<(), Error> {
        match container_exists(ROOT_DIR, self.id.as_str()) {
            Ok(exists) => {
                if !exists {
                    return Ok(());
                }
            }
            Err(err) => {
                error!("could not find the container, skipping cleanup: {}", err);
                return Ok(());
            }
        }
        match load_container(ROOT_DIR, self.id.as_str()) {
            Ok(mut container) => container.delete(true)?,
            Err(err) => {
                error!("could not find the container, skipping cleanup: {}", err);
                return Ok(());
            }
        }

        Ok(())
    }

    fn wait(&self, channel: Sender<(u32, DateTime<Utc>)>) -> Result<(), Error> {
        let code = self.exit_code.clone();
        thread::spawn(move || {
            let (lock, cvar) = &*code;
            let mut exit = lock.lock().unwrap();
            while (*exit).is_none() {
                exit = cvar.wait(exit).unwrap();
            }
            let ec = (*exit).unwrap();
            channel.send(ec).unwrap();
        });

        Ok(())
    }
}

#[cfg(test)]
mod wasitest {
    use std::borrow::Cow;
    use std::fs::{create_dir, read_to_string, File};
    use std::io::prelude::*;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
    use tempfile::{tempdir, TempDir};

    use serial_test::serial;

    use super::*;

    use wasmedge_sdk::{
        config::{CommonConfigOptions, ConfigBuilder},
        wat2wasm, Vm,
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

        let mut cfg = InstanceConfig::new(Wasi::new_engine()?);
        let cfg = cfg
            .set_bundle(dir.path().to_str().unwrap().to_string())
            .set_stdout(dir.path().join("stdout").to_str().unwrap().to_string());

        let wasi = Arc::new(Wasi::new("test".to_string(), Some(cfg)));

        wasi.start()?;

        let w = wasi.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            w.wait(tx).unwrap();
        });

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
    fn test_delete_after_create() {
        let config = ConfigBuilder::new(CommonConfigOptions::default())
            .build()
            .unwrap();
        let vm = Vm::new(Some(config)).unwrap();
        let i = Wasi::new("".to_string(), Some(&InstanceConfig::new(vm)));
        i.delete().unwrap();
    }

    #[test]
    #[serial]
    fn test_wasi() -> Result<(), Error> {
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

    // #[test]
    // #[serial]
    // fn test_wasi_error() -> Result<(), Error> {
    //     let dir = tempdir()?;
    //     let wasmbytes = wat2wasm(WASI_RETURN_ERROR).unwrap();
    //
    //     let res = run_wasi_test(&dir, wasmbytes)?;
    //
    //     // Expect error code from the run.
    //     assert_eq!(res.0, 137);
    //
    //     reset_stdio();
    //     Ok(())
    // }
}

impl EngineGetter for Wasi {
    type E = Vm;
    fn new_engine() -> Result<Vm, Error> {
        PluginManager::load_from_default_paths();
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
        let vm = Vm::new(Some(config)).map_err(anyhow::Error::msg)?;
        Ok(vm)
    }
}
