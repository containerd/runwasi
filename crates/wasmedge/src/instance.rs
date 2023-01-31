use anyhow::Context;
use chrono::{DateTime, Utc};
use containerd_shim_wasm::sandbox::error::Error;
use containerd_shim_wasm::sandbox::exec;
use containerd_shim_wasm::sandbox::oci;
use containerd_shim_wasm::sandbox::{EngineGetter, Instance, InstanceConfig};
use libc::{dup, dup2, SIGINT, SIGKILL, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use log::{debug, error};
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::os::unix::io::{IntoRawFd, RawFd};
use std::path::Path;
use std::sync::{
    mpsc::Sender,
    {Arc, Condvar, Mutex},
};
use std::thread;

use wasmedge_sdk::{
    config::{CommonConfigOptions, ConfigBuilder, HostRegistrationConfigOptions},
    params, PluginManager, Vm,
};

use super::error::WasmRuntimeError;
use super::oci_utils;

static mut STDIN_FD: Option<RawFd> = None;
static mut STDOUT_FD: Option<RawFd> = None;
static mut STDERR_FD: Option<RawFd> = None;

type ExitCode = (Mutex<Option<(u32, DateTime<Utc>)>>, Condvar);
pub struct Wasi {
    exit_code: Arc<ExitCode>,
    engine: Vm,

    stdin: String,
    stdout: String,
    stderr: String,
    bundle: String,

    pidfd: Arc<Mutex<Option<exec::PidFD>>>,
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
pub fn maybe_open_stdio(path: &str) -> Result<Option<RawFd>, Error> {
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

pub fn prepare_module(
    mut vm: Vm,
    spec: &oci::Spec,
    stdin_path: String,
    stdout_path: String,
    stderr_path: String,
) -> Result<Vm, WasmRuntimeError> {
    debug!("opening rootfs");
    let rootfs_path = oci::get_root(spec).to_str().unwrap();
    let root = format!("/:{}", rootfs_path);
    let mut preopens = vec![root.as_str()];

    debug!("opening mounts");
    let mut mounts = oci_utils::get_wasm_mounts(spec);
    preopens.append(&mut mounts);

    let args = oci::get_args(spec);
    let envs = oci_utils::env_to_wasi(spec);

    debug!("setting up wasi");
    let mut wasi_instance = vm.wasi_module()?;
    wasi_instance.initialize(
        Some(args.iter().map(|s| s as &str).collect()),
        Some(envs.iter().map(|s| s as &str).collect()),
        Some(preopens),
    );

    debug!("opening stdin");
    let stdin = maybe_open_stdio(&stdin_path).context("could not open stdin")?;
    if stdin.is_some() {
        unsafe {
            STDIN_FD = Some(dup(STDIN_FILENO));
            dup2(stdin.unwrap(), STDIN_FILENO);
        }
    }

    debug!("opening stdout");
    let stdout = maybe_open_stdio(&stdout_path).context("could not open stdout")?;
    if stdout.is_some() {
        unsafe {
            STDOUT_FD = Some(dup(STDOUT_FILENO));
            dup2(stdout.unwrap(), STDOUT_FILENO);
        }
    }

    debug!("opening stderr");
    let stderr = maybe_open_stdio(&stderr_path).context("could not open stderr")?;
    if stderr.is_some() {
        unsafe {
            STDERR_FD = Some(dup(STDERR_FILENO));
            dup2(stderr.unwrap(), STDERR_FILENO);
        }
    }

    let mut cmd = args[0].clone();
    let stripped = args[0].strip_prefix(std::path::MAIN_SEPARATOR);
    if let Some(strpd) = stripped {
        cmd = strpd.to_string();
    }

    let mod_path = oci::get_root(spec).join(cmd);

    debug!("register module from file");
    let vm = vm.register_module_from_file("main", mod_path)?;

    Ok(vm)
}

impl Instance for Wasi {
    type E = Vm;
    fn new(_id: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self {
        let cfg = cfg.unwrap(); // TODO: handle error
        Wasi {
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
        let engine = self.engine.clone();
        let stdin = self.stdin.clone();
        let stdout = self.stdout.clone();
        let stderr = self.stderr.clone();

        debug!("preparing module");
        let spec = load_spec(self.bundle.clone())?;

        debug!("call prehook before the start");
        oci::setup_prestart_hooks(spec.hooks())?;

        let vm = prepare_module(engine, &spec, stdin, stdout, stderr)
            .map_err(|e| Error::Others(format!("error setting up module: {}", e)))?;

        let cg = oci::get_cgroup(&spec)?;

        oci::setup_cgroup(cg.as_ref(), &spec)
            .map_err(|e| Error::Others(format!("error setting up cgroups: {}", e)))?;
        let res = unsafe { exec::fork(Some(cg.as_ref())) }?;
        match res {
            exec::Context::Parent(tid, pidfd) => {
                let mut lr = self.pidfd.lock().unwrap();
                *lr = Some(pidfd.clone());

                debug!("started wasi instance with tid {}", tid);

                let code = self.exit_code.clone();

                let _ = thread::spawn(move || {
                    let (lock, cvar) = &*code;
                    let status = match pidfd.wait() {
                        Ok(status) => status,
                        Err(e) => {
                            error!("error waiting for pid {}: {}", tid, e);
                            cvar.notify_all();
                            return;
                        }
                    };

                    debug!("wasi instance exited with status {}", status.status);
                    let mut ec = lock.lock().unwrap();
                    *ec = Some((status.status, Utc::now()));
                    drop(ec);
                    cvar.notify_all();
                });
                Ok(tid)
            }
            exec::Context::Child => {
                // child process

                // TODO: How to get exit code?
                // This was relatively straight forward in go, but wasi and wasmtime are totally separate things in rust.
                let _ret = match vm.run_func(Some("main"), "_start", params!()) {
                    Ok(_) => std::process::exit(0),
                    Err(_) => std::process::exit(137),
                };
            }
        }
    }

    fn kill(&self, signal: u32) -> Result<(), Error> {
        if signal as i32 != SIGKILL && signal as i32 != SIGINT {
            println!("{:?}", signal);
            return Err(Error::InvalidArgument(
                "only SIGKILL and SIGINT are supported".to_string(),
            ));
        }

        let lr = self.pidfd.lock().unwrap();
        let fd = lr
            .as_ref()
            .ok_or_else(|| Error::FailedPrecondition("module is not running".to_string()))?;
        fd.kill(SIGKILL as i32)
    }

    fn delete(&self) -> Result<(), Error> {
        let spec = match load_spec(self.bundle.clone()) {
            Ok(spec) => spec,
            Err(err) => {
                error!("Could not load spec, skipping cgroup cleanup: {}", err);
                return Ok(());
            }
        };
        let cg = oci::get_cgroup(&spec)?;
        cg.delete()?;
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
    use std::fs::{create_dir, read_to_string, File};
    use std::io::prelude::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
    use tempfile::tempdir;

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
    fn test_wasi() -> Result<(), Error> {
        let dir = tempdir()?;
        create_dir(dir.path().join("rootfs"))?;

        let wasmbytes = wat2wasm(WASI_HELLO_WAT).unwrap();
        let mut f = File::create(dir.path().join("rootfs/hello.wasm"))?;
        f.write_all(&wasmbytes)?;

        let stdout = File::create(dir.path().join("stdout"))?;
        drop(stdout);

        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec!["hello.wasm".to_string()])
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

        reset_stdio();
        Ok(())
    }
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
