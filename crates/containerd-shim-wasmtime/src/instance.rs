use anyhow::{bail, Result};
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::{Container, ContainerStatus};
use nix::errno::Errno;
use nix::sys::wait::waitid;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::os::fd::{IntoRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use anyhow::Context;
use chrono::{DateTime, Utc};
use containerd_shim_wasm::sandbox::error::Error;
use containerd_shim_wasm::sandbox::instance::Wait;
use containerd_shim_wasm::sandbox::{oci, EngineGetter, Instance, InstanceConfig};
use libc::{SIGINT, SIGKILL};
use libcontainer::syscall::syscall::create_syscall;
use log::error;
use nix::sys::wait::{Id as WaitID, WaitPidFlag, WaitStatus};
use serde::{Deserialize, Serialize};
use wasmtime::Engine;

use crate::executor::WasmtimeExecutor;
use libcontainer::signal::Signal;

type ExitCode = Arc<(Mutex<Option<(u32, DateTime<Utc>)>>, Condvar)>;

pub struct Wasi {
    exit_code: ExitCode,
    engine: wasmtime::Engine,

    stdin: String,
    stdout: String,
    stderr: String,
    bundle: String,

    rootdir: PathBuf,

    id: String,
}

fn construct_container_root<P: AsRef<Path>>(root_path: P, container_id: &str) -> Result<PathBuf> {
    let root_path = fs::canonicalize(&root_path).with_context(|| {
        format!(
            "failed to canonicalize {} for container {}",
            root_path.as_ref().display(),
            container_id
        )
    })?;
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

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

fn load_spec(bundle: String) -> Result<oci::Spec, Error> {
    let mut spec = oci::load(Path::new(&bundle).join("config.json").to_str().unwrap())?;
    spec.canonicalize_rootfs(&bundle)
        .map_err(|e| Error::Others(format!("error canonicalizing rootfs in spec: {}", e)))?;
    Ok(spec)
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

impl Instance for Wasi {
    type E = wasmtime::Engine;
    fn new(id: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self {
        // TODO: there are failure cases e.x. parsing cfg, loading spec, etc.
        // thus should make `new` return `Result<Self, Error>` instead of `Self`
        log::info!("creating new instance: {}", id);
        let cfg = cfg.unwrap();
        let bundle = cfg.get_bundle().unwrap_or_default();
        let spec = load_spec(bundle.clone()).unwrap();
        let rootdir = oci::get_root(&spec);
        Wasi {
            id,
            exit_code: Arc::new((Mutex::new(None), Condvar::new())),
            engine: cfg.get_engine(),
            stdin: cfg.get_stdin().unwrap_or_default(),
            stdout: cfg.get_stdout().unwrap_or_default(),
            stderr: cfg.get_stderr().unwrap_or_default(),
            bundle,
            rootdir: rootdir.clone(),
        }
    }
    fn start(&self) -> Result<u32, Error> {
        log::info!("starting instance: {}", self.id);
        let engine = self.engine.clone();

        let mut container = self.build_container(
            self.stdin.as_str(),
            self.stdout.as_str(),
            self.stderr.as_str(),
            engine,
        )?;

        log::info!("created container: {}", self.id);
        let code = self.exit_code.clone();
        let pid = container.pid().unwrap();

        container
            .start()
            .map_err(|err| Error::Any(anyhow::anyhow!("failed to start container: {}", err)))?;

        thread::spawn(move || {
            let (lock, cvar) = &*code;

            let status = match waitid(WaitID::Pid(pid), WaitPidFlag::WEXITED) {
                Ok(WaitStatus::Exited(_, status)) => status,
                Ok(WaitStatus::Signaled(_, sig, _)) => sig as i32,
                Ok(_) => 0,
                Err(e) => {
                    if e == Errno::ECHILD {
                        0
                    } else {
                        panic!("waitpid failed: {}", e);
                    }
                }
            } as u32;
            let mut ec = lock.lock().unwrap();
            *ec = Some((status, Utc::now()));
            drop(ec);
            cvar.notify_all();
        });

        Ok(pid.as_raw() as u32)
    }

    fn kill(&self, signal: u32) -> Result<(), Error> {
        log::info!("killing instance: {}", self.id);
        if signal as i32 != SIGKILL && signal as i32 != SIGINT {
            return Err(Error::InvalidArgument(
                "only SIGKILL and SIGINT are supported".to_string(),
            ));
        }

        let mut container = load_container(&self.rootdir, self.id.as_str())?;
        let signal = Signal::try_from(signal as i32)
            .map_err(|err| Error::InvalidArgument(format!("invalid signal number: {}", err)))?;
        match container.kill(signal, true) {
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
        log::info!("deleting instance: {}", self.id);
        match container_exists(&self.rootdir, self.id.as_str()) {
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
        match load_container(&self.rootdir, self.id.as_str()) {
            Ok(mut container) => container.delete(true).map_err(|err| {
                Error::Any(anyhow::anyhow!(
                    "failed to delete container {}: {}",
                    self.id,
                    err
                ))
            })?,
            Err(err) => {
                error!("could not find the container, skipping cleanup: {}", err);
                return Ok(());
            }
        }

        Ok(())
    }

    fn wait(&self, waiter: &Wait) -> Result<(), Error> {
        log::info!("waiting for instance: {}", self.id);
        let code = self.exit_code.clone();
        waiter.set_up_exit_code_wait(code)
    }
}

impl Wasi {
    fn build_container(
        &self,
        stdin: &str,
        stdout: &str,
        stderr: &str,
        engine: Engine,
    ) -> anyhow::Result<Container> {
        let syscall = create_syscall();
        let stdin = maybe_open_stdio(stdin).context("could not open stdin")?;
        let stdout = maybe_open_stdio(stdout).context("could not open stdout")?;
        let stderr = maybe_open_stdio(stderr).context("could not open stderr")?;

        let container = ContainerBuilder::new(self.id.clone(), syscall.as_ref())
            .with_executor(vec![Box::new(WasmtimeExecutor {
                stdin,
                stdout,
                stderr,
                engine,
            })])?
            .with_root_path(self.rootdir.clone())?
            .as_init(&self.bundle)
            .with_systemd(false)
            .build()?;
        Ok(container)
    }
}

#[cfg(test)]
mod wasitest {
    use std::fs::{create_dir, read_to_string, File};
    use std::io::prelude::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use containerd_shim_wasm::function;
    use containerd_shim_wasm::sandbox::exec::has_cap_sys_admin;
    use containerd_shim_wasm::sandbox::instance::Wait;
    use containerd_shim_wasm::sandbox::testutil::run_test_with_sudo;
    use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
    use tempfile::{tempdir, TempDir};

    use super::*;

    // This is taken from https://github.com/bytecodealliance/wasmtime/blob/6a60e8363f50b936e4c4fc958cb9742314ff09f3/docs/WASI-tutorial.md?plain=1#L270-L298
    const WASI_HELLO_WAT: &[u8]= r#"(module
        ;; Import the required fd_write WASI function which will write the given io vectors to stdout
        ;; The function signature for fd_write is:
        ;; (File Descriptor, *iovs, iovs_len, nwritten) -> Returns number of bytes written
        (import "wasi_unstable" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))

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

    #[test]
    fn test_delete_after_create() -> Result<()> {
        let dir = tempdir()?;
        create_dir(dir.path().join("rootfs"))?;
        let cfg = prepare_cfg(&dir)?;

        let i = Wasi::new("".to_string(), Some(&cfg));
        i.delete()?;
        Ok(())
    }

    #[test]
    fn test_wasi() -> Result<(), Error> {
        if !has_cap_sys_admin() {
            println!("running test with sudo: {}", function!());
            return run_test_with_sudo(function!());
        }
        let dir = tempdir()?;
        create_dir(dir.path().join("rootfs"))?;
        let cfg = prepare_cfg(&dir)?;

        let wasi = Wasi::new("test".to_string(), Some(&cfg));

        wasi.start()?;

        let (tx, rx) = channel();
        let waiter = Wait::new(tx);
        wasi.wait(&waiter).unwrap();

        let res = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(res) => res,
            Err(e) => {
                wasi.kill(SIGKILL as u32).unwrap();
                return Err(Error::Others(format!(
                    "error waiting for module to finish: {0}",
                    e
                )));
            }
        };
        assert_eq!(res.0, 0);

        let output = read_to_string(dir.path().join("stdout"))?;
        assert_eq!(output, "hello world\n");

        wasi.delete()?;

        Ok(())
    }

    fn prepare_cfg(dir: &TempDir) -> Result<InstanceConfig<Engine>> {
        let mut f = File::create(dir.path().join("rootfs/hello.wat"))?;
        f.write_all(WASI_HELLO_WAT)?;
        let stdout = File::create(dir.path().join("stdout"))?;
        drop(stdout);
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec!["hello.wat".to_string()])
                    .build()?,
            )
            .build()?;
        spec.save(dir.path().join("config.json"))?;
        let mut cfg = InstanceConfig::new(Engine::default(), "test_namespace".into());
        let cfg = cfg
            .set_bundle(dir.path().to_str().unwrap().to_string())
            .set_stdout(dir.path().join("stdout").to_str().unwrap().to_string());
        Ok(cfg.to_owned())
    }
}

impl EngineGetter for Wasi {
    type E = wasmtime::Engine;
    fn new_engine() -> Result<Engine, Error> {
        let engine = Engine::default();
        Ok(engine)
    }
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
