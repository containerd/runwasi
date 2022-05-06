use super::error::Error;
use super::oci;
use anyhow::Context;
use chrono::{DateTime, Utc};
use log::{debug, error};
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::mpsc::channel;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use wasmtime::{Config as EngineConfig, Engine, Linker, Module, Store};
use wasmtime_wasi::{sync::file::File as WasiFile, WasiCtx, WasiCtxBuilder};

#[derive(Clone)]
pub struct InstanceConfig<E: Send + Sync + Clone> {
    engine: E,
    stdin: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
    bundle: Option<String>,
}

impl<E> InstanceConfig<E>
where
    E: Send + Sync + Clone,
{
    pub fn new(engine: E) -> Self {
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

    pub fn get_engine(&self) -> E {
        self.engine.clone()
    }
}

pub trait Instance {
    type E: Send + Sync + Clone;
    fn new(id: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self;
    fn start(&self) -> Result<u32, Error>;
    fn kill(&self, signal: u32) -> Result<(), Error>;
    fn delete(&self) -> Result<(), Error>;
    fn wait(&self, send: Sender<(u32, DateTime<Utc>)>) -> Result<(), Error>;
}

pub struct Wasi {
    interupt: Arc<RwLock<Option<wasmtime::InterruptHandle>>>,
    exit_code: Arc<(Mutex<Option<(u32, DateTime<Utc>)>>, Condvar)>,
    engine: wasmtime::Engine,

    id: String,
    stdin: String,
    stdout: String,
    stderr: String,
    bundle: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_maybe_open_stdio() -> Result<(), Error> {
        let f = maybe_open_stdio("")?;
        assert!(f.is_none());

        let f = maybe_open_stdio("/some/nonexistent/path")?;
        assert!(f.is_none());

        let dir = tempdir()?;
        let temp = File::create(dir.path().join("testfile"))?;
        drop(temp);
        let f = maybe_open_stdio(&dir.path().join("testfile").as_path().to_str().unwrap())?;
        assert!(f.is_some());
        drop(f);

        Ok(())
    }
}

/// containerd can send an empty path or a non-existant path
/// In both these cases we should just assume that the stdio stream was not setup (intentionally)
/// Any other error is a real error.
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
    debug!("opening rootfs");
    let rootfs = oci::get_rootfs(&spec)?;
    let args = oci::get_args(&spec);
    let env = oci::env_to_wasi(&spec);

    debug!("setting up wasi");
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

    let mod_path = oci::get_root(&spec).join(cmd);

    debug!("loading module from file");
    let module = Module::from_file(&engine, mod_path)
        .map_err(|err| Error::Others(format!("could not load module from file: {}", err)))?;

    Ok((wctx, module))
}

impl Instance for Wasi {
    type E = wasmtime::Engine;
    fn new(id: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self {
        let cfg = cfg.unwrap(); // TODO: handle error
        Wasi {
            interupt: Arc::new(RwLock::new(None)),
            exit_code: Arc::new((Mutex::new(None), Condvar::new())),
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
        let (tx, rx) = channel::<Result<(), Error>>();
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
                            let mut ec = lock.lock().unwrap();
                            *ec = Some((0, Utc::now()));
                        }
                        Err(_) => {
                            error!("exit code: {}", 137);
                            let mut ec = lock.lock().unwrap();
                            *ec = Some((137, Utc::now()));
                        }
                    };

                    cvar.notify_all();
                })?;

        debug!("Waiting for start notification");
        match rx.recv().unwrap() {
            Ok(_) => (),
            Err(err) => {
                debug!("error starting instance: {}", err);
                let code = self.exit_code.clone();

                let (lock, cvar) = &*code;
                let mut ec = lock.lock().unwrap();
                *ec = Some((139, Utc::now()));
                cvar.notify_all();
                return Err(err);
            }
        }

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
    use super::*;
    use std::fs::{create_dir, read_to_string, write, File};
    use std::io::prelude::*;
    use std::time::Duration;
    use tempfile::tempdir;
    use wasmtime::Config;

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
    fn test_delete_after_create() {
        let i = Wasi::new(
            "".to_string(),
            Some(&InstanceConfig::new(Engine::default())),
        );
        i.delete().unwrap();
    }

    #[test]
    fn test_wasi() -> Result<(), Error> {
        let dir = tempdir()?;
        create_dir(&dir.path().join("rootfs"))?;

        let mut f = File::create(dir.path().join("rootfs/hello.wat"))?;
        f.write_all(WASI_HELLO_WAT)?;

        let stdout = File::create(dir.path().join("stdout"))?;
        drop(stdout);

        write(
            dir.path().join("config.json"),
            "{
                \"root\": {
                    \"path\": \"rootfs\"
                },
                \"process\":{
                    \"cwd\": \"/\",
                    \"args\": [\"hello.wat\"],
                    \"user\": {
                        \"uid\": 0,
                        \"gid\": 0
                    }
                }
            }"
            .as_bytes(),
        )?;

        let mut cfg = InstanceConfig::new(Engine::new(Config::new().interruptable(true))?);
        let cfg = cfg
            .set_bundle(dir.path().to_str().unwrap().to_string())
            .set_stdout(dir.path().join("stdout").to_str().unwrap().to_string());

        let wasi = Arc::new(Wasi::new("test".to_string(), cfg));

        wasi.start()?;

        let w = wasi.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            w.wait(tx).unwrap();
        });

        let res = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(res) => res,
            Err(e) => {
                wasi.kill(9).unwrap();
                return Err(Error::Others(format!(
                    "error waiting for module to finish: {0}",
                    e
                )));
            }
        };
        assert_eq!(res.0, 0);

        let output = read_to_string(dir.path().join("stdout"))?;
        assert_eq!(output, "hello world\n");

        Ok(())
    }
}

// This is used for the "pause" container with cri.
pub struct Nop {
    // Since we are faking the container, we need to keep track of the "exit" code/time
    // We'll just mark it as exited when kill is called.
    exit_code: Arc<(Mutex<Option<(u32, DateTime<Utc>)>>, Condvar)>,
}

impl Instance for Nop {
    type E = wasmtime::Engine;
    fn new(_id: String, _cfg: Option<&InstanceConfig<Self::E>>) -> Self {
        Nop {
            exit_code: Arc::new((Mutex::new(None), Condvar::new())),
        }
    }
    fn start(&self) -> Result<u32, Error> {
        Ok(std::process::id())
    }

    fn kill(&self, signal: u32) -> Result<(), Error> {
        let code = match signal {
            9 => 137,
            2 | 15 => 0,
            s => {
                return Err(Error::InvalidArgument(format!("unsupported signal: {}", s)));
            }
        };

        let exit_code = self.exit_code.clone();
        let (lock, cvar) = &*exit_code;
        let mut lock = lock.lock().unwrap();
        *lock = Some((code, Utc::now()));
        cvar.notify_all();

        Ok(())
    }
    fn delete(&self) -> Result<(), Error> {
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
mod noptests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_nop_kill_sigkill() -> Result<(), Error> {
        let nop = Arc::new(Nop::new("".to_string(), None));
        let (tx, rx) = channel();

        let n = nop.clone();

        thread::spawn(move || {
            n.wait(tx).unwrap();
        });

        nop.kill(9)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 137);
        Ok(())
    }

    #[test]
    fn test_nop_kill_sigterm() -> Result<(), Error> {
        let nop = Arc::new(Nop::new("".to_string(), None));
        let (tx, rx) = channel();

        let n = nop.clone();

        thread::spawn(move || {
            n.wait(tx).unwrap();
        });

        nop.kill(15)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 0);
        Ok(())
    }

    #[test]
    fn test_nop_kill_sigint() -> Result<(), Error> {
        let nop = Arc::new(Nop::new("".to_string(), None));
        let (tx, rx) = channel();

        let n = nop.clone();

        thread::spawn(move || {
            n.wait(tx).unwrap();
        });

        nop.kill(2)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 0);
        Ok(())
    }

    #[test]
    fn test_op_kill_other() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None);

        let err = nop.kill(1).unwrap_err();
        match err {
            Error::InvalidArgument(_) => {}
            _ => panic!("unexpected error: {}", err),
        }

        Ok(())
    }

    #[test]
    fn test_nop_delete_after_create() {
        let nop = Nop::new("".to_string(), None);
        nop.delete().unwrap();
    }
}

pub trait EngineGetter {
    type E: Send + Sync + Clone;
    fn new_engine() -> Result<Self::E, Error>;
}

impl EngineGetter for Wasi {
    type E = wasmtime::Engine;
    fn new_engine() -> Result<Engine, Error> {
        let engine = Engine::new(EngineConfig::default().interruptable(true))?;
        Ok(engine)
    }
}
