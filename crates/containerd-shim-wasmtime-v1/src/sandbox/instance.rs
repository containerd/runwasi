use super::error::Error;
use super::oci;
use anyhow::Context;
use chrono::{DateTime, Utc};
use log::{debug, error, info};
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::{sync::file::File as WasiFile, WasiCtx, WasiCtxBuilder};

#[derive(Clone)]
pub struct InstanceConfig {
    engine: Engine,
    stdin: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
    bundle: Option<String>,
}

impl InstanceConfig {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            stdin: None,
            stdout: None,
            stderr: None,
            bundle: None,
        }
    }

    pub fn set_stdin(&mut self, stdin: String) -> &mut Self {
        self.stdin = Some(stdin);
        self
    }

    pub fn get_stdin(&self) -> Option<String> {
        self.stdin.clone()
    }

    pub fn set_stdout(&mut self, stdout: String) -> &mut Self {
        self.stdout = Some(stdout);
        self
    }

    pub fn get_stdout(&self) -> Option<String> {
        self.stdout.clone()
    }

    pub fn set_stderr(&mut self, stderr: String) -> &mut Self {
        self.stderr = Some(stderr);
        self
    }

    pub fn get_stderr(&self) -> Option<String> {
        self.stderr.clone()
    }

    pub fn set_bundle(&mut self, bundle: String) -> &mut Self {
        self.bundle = Some(bundle);
        self
    }

    pub fn get_bundle(&self) -> Option<String> {
        self.bundle.clone()
    }
}

pub trait Instance {
    fn new(id: String, cfg: &InstanceConfig) -> Self;
    fn start(&self) -> Result<u32, Error>;
    fn kill(&self, signal: u32) -> Result<(), Error>;
    fn delete(&self) -> Result<(), Error>;
    fn wait(&self) -> Result<(u32, DateTime<Utc>), Error>;
}

pub struct Wasi {
    interupt: Arc<RwLock<Option<wasmtime::InterruptHandle>>>,
    exit_code: Arc<(Mutex<(Option<u32>, Option<DateTime<Utc>>)>, Condvar)>,
    engine: wasmtime::Engine,

    id: String,
    stdin: String,
    stdout: String,
    stderr: String,
    bundle: String,
}

// containerd can send an empty path or a non-existant path
// In both these cases we should just assume that the stdio stream was not setup (intentionally)
// Any other error is a real error.
pub fn maybe_open_stdio(path: &str) -> Result<Option<WasiFile>, Error> {
    if path.is_empty() {
        return Ok(None);
    }
    match oci::wasi_file(path, OpenOptions::new().read(true).write(true)) {
        Ok(f) => Ok(Some(f)),
        Err(err) => match err.kind() {
            std::io::ErrorKind::NotFound => Ok(None),
            _ => Err(err.into()),
        },
    }
}

pub fn prepare_module(
    engine: wasmtime::Engine,
    bundle: String,
    stdin_path: String,
    stdout_path: String,
    stderr_path: String,
) -> Result<(WasiCtx, Module), Error> {
    let mut spec = oci::load(Path::new(&bundle).join("config.json").to_str().unwrap())?;

    spec.canonicalize_rootfs(&bundle)
        .map_err(|err| Error::Others(format!("could not canonicalize rootfs: {}", err)))?;
    let root = match spec.root() {
        Some(r) => r.path(),
        None => {
            return Err(Error::Others(
                "rootfs is not specified in the config.json".to_string(),
            ));
        }
    };

    debug!("opening rootfs");
    let rootfs = oci::wasi_dir(root.to_str().unwrap(), OpenOptions::new().read(true))
        .map_err(|err| Error::Others(format!("could not open rootfs: {}", err)))?;
    let args = oci::get_args(&spec);
    let env = oci::env_to_wasi(&spec);

    let mut wasi_builder = WasiCtxBuilder::new()
        .args(args)?
        .envs(env.as_slice())?
        .preopened_dir(rootfs, "/")?;

    debug!("opening stdin");
    let stdin = maybe_open_stdio(&stdin_path).context("could not open stdin")?;
    if stdin.is_some() {
        wasi_builder = wasi_builder.stdin(Box::new(stdin.unwrap()));
    }

    debug!("opening stdout");
    let stdout = maybe_open_stdio(&stdout_path).context("could not open stdout")?;
    if stdout.is_some() {
        wasi_builder = wasi_builder.stdout(Box::new(stdout.unwrap()));
    }

    debug!("opening stderr");
    let stderr = maybe_open_stdio(&stderr_path).context("could not open stderr")?;
    if stderr.is_some() {
        wasi_builder = wasi_builder.stderr(Box::new(stderr.unwrap()));
    }

    debug!("building wasi context");
    let wctx = wasi_builder.build();
    debug!("wasi context ready");

    let mut cmd = args[0].clone();
    let stripped = args[0].strip_prefix(std::path::MAIN_SEPARATOR);
    if stripped.is_some() {
        cmd = stripped.unwrap().to_string();
    }

    let mod_path = root.join(cmd);

    debug!("loading module from file");
    let module = Module::from_file(&engine, mod_path)
        .map_err(|err| Error::Others(format!("could not load module from file: {}", err)))?;

    Ok((wctx, module))
}

impl Instance for Wasi {
    fn new(id: String, cfg: &InstanceConfig) -> Self {
        Wasi {
            interupt: Arc::new(RwLock::new(None)),
            exit_code: Arc::new((Mutex::new((None, None)), Condvar::new())),
            engine: cfg.engine.clone(),
            id,
            stdin: cfg.get_stdin().unwrap_or_default(),
            stdout: cfg.get_stdout().unwrap_or_default(),
            stderr: cfg.get_stderr().unwrap_or_default(),
            bundle: cfg.get_bundle().unwrap_or_default(),
        }
    }
    fn start(&self) -> Result<u32, Error> {
        let engine = self.engine.clone();

        let exit_code = self.exit_code.clone();
        let interupt = self.interupt.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), Error>>();
        let bundle = self.bundle.clone();
        let stdin = self.stdin.clone();
        let stdout = self.stdout.clone();
        let stderr = self.stderr.clone();

        let _ =
            thread::Builder::new()
                .name(self.id.clone())
                .spawn(move || {
                    debug!("starting instance");
                    let mut linker = Linker::new(&engine);

                    match wasmtime_wasi::add_to_linker(&mut linker, |s| s)
                        .map_err(|err| Error::Others(format!("error adding to linker: {}", err)))
                    {
                        Ok(_) => (),
                        Err(err) => {
                            tx.send(Err(err)).unwrap();
                            return;
                        }
                    };

                    debug!("preparing module");
                    let m = match prepare_module(engine.clone(), bundle, stdin, stdout, stderr) {
                        Ok(f) => f,
                        Err(err) => {
                            tx.send(Err(err)).unwrap();
                            return;
                        }
                    };

                    let mut store = Store::new(&engine, m.0);

                    debug!("instantiating instnace");
                    let i = match linker.instantiate(&mut store, &m.1).map_err(|err| {
                        Error::Others(format!("error instantiating module: {}", err))
                    }) {
                        Ok(i) => i,
                        Err(err) => {
                            tx.send(Err(err)).unwrap();
                            return;
                        }
                    };

                    debug!("getting interupt handle");
                    match store.interrupt_handle().map_err(|err| {
                        Error::Others(format!("could not get interupt handle: {}", err))
                    }) {
                        Ok(h) => {
                            let mut lock = interupt.write().unwrap();
                            *lock = Some(h);
                            drop(lock);
                        }
                        Err(err) => {
                            tx.send(Err(err)).unwrap();
                            return;
                        }
                    };

                    debug!("getting start function");
                    let f = match i
                        .get_func(&mut store, "_start")
                        .ok_or(Error::InvalidArgument(
                            "module does not have a wasi start function".to_string(),
                        )) {
                        Ok(f) => f,
                        Err(err) => {
                            tx.send(Err(err)).unwrap();
                            return;
                        }
                    };

                    debug!("notifying main thread we are about to start");
                    tx.send(Ok(())).unwrap();

                    debug!("starting wasi instance");

                    // TODO: How to get exit code?
                    // This was relatively straight forward in go, but wasi and wasmtime are totally separate things in rust.
                    let (lock, cvar) = &*exit_code;
                    let _ret = match f.call(&mut store, &mut vec![], &mut vec![]) {
                        Ok(_) => {
                            debug!("exit code: {}", 0);
                            let mut lock = lock.lock().unwrap();
                            *lock = (Some(0), Some(Utc::now()));
                        }
                        Err(_) => {
                            error!("exit code: {}", 137);
                            let mut lock = lock.lock().unwrap();
                            *lock = (Some(137), Some(Utc::now()));
                        }
                    };

                    cvar.notify_all();
                })?;

        debug!("Waiting for start notification");
        rx.recv().unwrap()?;

        Ok(1) // TODO: PID: I wanted to use a thread ID here, but threads use a u64, the API wants a u32
    }

    fn kill(&self, signal: u32) -> Result<(), Error> {
        if signal != 9 {
            return Err(Error::InvalidArgument(
                "only SIGKILL is supported".to_string(),
            ));
        }

        let interupt = self.interupt.read().unwrap();
        let i = interupt.as_ref().ok_or(Error::FailedPrecondition(
            "module is not running".to_string(),
        ))?;

        i.interrupt();
        Ok(())
    }

    fn delete(&self) -> Result<(), Error> {
        Ok(())
    }

    fn wait(&self) -> Result<(u32, DateTime<Utc>), Error> {
        let (lock, cvar) = &*self.exit_code;
        let mut exit = lock.lock().unwrap();
        while (*exit).0.is_none() {
            exit = cvar.wait(exit).unwrap();
        }

        Ok(((*exit).0.unwrap(), (*exit).1.unwrap()))
    }
}

// This is used for the "pause" container with cri.
pub struct Nop {
    // Since we are faking the container, we need to keep track of the "exit" code/time
    // We'll just mark it as exited when kill is called.
    exit_code: Arc<(Mutex<(Option<u32>, Option<DateTime<Utc>>)>, Condvar)>,
}

impl Instance for Nop {
    fn new(_id: String, _cfg: &InstanceConfig) -> Self {
        Nop {
            exit_code: Arc::new((Mutex::new((None, None)), Condvar::new())),
        }
    }
    fn start(&self) -> Result<u32, Error> {
        Ok(0) // TODO: PID
    }

    fn kill(&self, signal: u32) -> Result<(), Error> {
        let code = match signal {
            9 => 137,
            2 | 15 => 0,
            s => {
                return Err(Error::Others(format!("unsupported signal: {}", s)));
            }
        };

        let exit_code = self.exit_code.clone();
        let (lock, cvar) = &*exit_code;
        let mut lock = lock.lock().unwrap();
        *lock = (Some(code), Some(Utc::now()));
        cvar.notify_all();

        Ok(())
    }
    fn delete(&self) -> Result<(), Error> {
        Ok(())
    }
    fn wait(&self) -> Result<(u32, DateTime<Utc>), Error> {
        let (lock, cvar) = &*self.exit_code;
        let mut exit = lock.lock().unwrap();
        while (*exit).0.is_none() {
            exit = cvar.wait(exit).unwrap();
        }

        Ok(((*exit).0.unwrap(), (*exit).1.unwrap()))
    }
}
