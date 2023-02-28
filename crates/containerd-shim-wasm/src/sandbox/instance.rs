//! Abstractions for running/managing a wasm/wasi instance.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use libc::{SIGINT, SIGKILL, SIGTERM};

use chrono::{DateTime, Utc};

use super::error::Error;

type ExitCode = (Mutex<Option<(u32, DateTime<Utc>)>>, Condvar);

/// Generic options builder for creating a wasm instance.
/// This is passed to the `Instance::new` method.
#[derive(Clone)]
pub struct InstanceConfig<E>
where
    E: Send + Sync + Clone,
{
    /// The wasm engine to use.
    /// This should be cheap to clone.
    engine: E,
    /// Optional stdin named pipe path.
    stdin: Option<String>,
    /// Optional stdout named pipe path.
    stdout: Option<String>,
    /// Optional stderr named pipe path.
    stderr: Option<String>,
    /// Path to the OCI bundle directory.
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

    /// set the stdin path for the instance
    pub fn set_stdin(&mut self, stdin: String) -> &mut Self {
        self.stdin = Some(stdin);
        self
    }

    /// get the stdin path for the instance
    pub fn get_stdin(&self) -> Option<String> {
        self.stdin.clone()
    }

    /// set the stdout path for the instance
    pub fn set_stdout(&mut self, stdout: String) -> &mut Self {
        self.stdout = Some(stdout);
        self
    }

    /// get the stdout path for the instance
    pub fn get_stdout(&self) -> Option<String> {
        self.stdout.clone()
    }

    /// set the stderr path for the instance
    pub fn set_stderr(&mut self, stderr: String) -> &mut Self {
        self.stderr = Some(stderr);
        self
    }

    /// get the stderr path for the instance
    pub fn get_stderr(&self) -> Option<String> {
        self.stderr.clone()
    }

    /// set the OCI bundle path for the instance
    pub fn set_bundle(&mut self, bundle: String) -> &mut Self {
        self.bundle = Some(bundle);
        self
    }

    /// get the OCI bundle path for the instance
    pub fn get_bundle(&self) -> Option<String> {
        self.bundle.clone()
    }

    /// get the wasm engine for the instance
    pub fn get_engine(&self) -> E {
        self.engine.clone()
    }
}

/// Represents a wasi module(s).
/// Instance is a trait that gets implemented by consumers of this library.
pub trait Instance {
    type E: Send + Sync + Clone;
    /// Create a new instance
    fn new(id: String, rootdir: String, cfg: Option<&InstanceConfig<Self::E>>) -> Self;
    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, Error>;
    /// Send a signal to the instance
    fn kill(&self, signal: u32) -> Result<(), Error>;
    /// delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), Error>;
    /// wait for the instance to exit
    /// The sender is used to send the exit code and time back to the caller
    /// Ideally this would just be a blocking call with a normal result, however
    /// because of how this is called from a thread it causes issues with lifetimes of the trait implementer.
    fn wait(&self, send: Sender<(u32, DateTime<Utc>)>) -> Result<(), Error>;
}

/// This is used for the "pause" container with cri and is a no-op instance implementation.
pub struct Nop {
    /// Since we are faking the container, we need to keep track of the "exit" code/time
    /// We'll just mark it as exited when kill is called.
    exit_code: Arc<ExitCode>,
}

impl Instance for Nop {
    type E = ();
    fn new(_id: String, _rootdir: String, _cfg: Option<&InstanceConfig<Self::E>>) -> Self {
        Nop {
            exit_code: Arc::new((Mutex::new(None), Condvar::new())),
        }
    }
    fn start(&self) -> Result<u32, Error> {
        Ok(std::process::id())
    }

    fn kill(&self, signal: u32) -> Result<(), Error> {
        let code = match signal as i32 {
            SIGKILL => 137,
            SIGINT | SIGTERM => 0,
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
    use std::sync::mpsc::channel;
    use std::sync::Arc;
    use std::time::Duration;

    use libc::SIGHUP;

    use super::*;

    #[test]
    fn test_nop_kill_sigkill() -> Result<(), Error> {
        let nop = Arc::new(Nop::new("".to_string(), "".into(), None));
        let (tx, rx) = channel();

        let n = nop.clone();

        thread::spawn(move || {
            n.wait(tx).unwrap();
        });

        nop.kill(SIGKILL as u32)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 137);
        Ok(())
    }

    #[test]
    fn test_nop_kill_sigterm() -> Result<(), Error> {
        let nop = Arc::new(Nop::new("".to_string(), "".into(), None));
        let (tx, rx) = channel();

        let n = nop.clone();

        thread::spawn(move || {
            n.wait(tx).unwrap();
        });

        nop.kill(SIGTERM as u32)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 0);
        Ok(())
    }

    #[test]
    fn test_nop_kill_sigint() -> Result<(), Error> {
        let nop = Arc::new(Nop::new("".to_string(), "".into(), None));
        let (tx, rx) = channel();

        let n = nop.clone();

        thread::spawn(move || {
            n.wait(tx).unwrap();
        });

        nop.kill(SIGINT as u32)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 0);
        Ok(())
    }

    #[test]
    fn test_op_kill_other() -> Result<(), Error> {
        let nop = Arc::new(Nop::new("".to_string(), "".into(), None));

        let err = nop.kill(SIGHUP as u32).unwrap_err();
        match err {
            Error::InvalidArgument(_) => {}
            _ => panic!("unexpected error: {}", err),
        }

        Ok(())
    }

    #[test]
    fn test_nop_delete_after_create() {
        let nop = Arc::new(Nop::new("".to_string(), "".into(), None));
        nop.delete().unwrap();
    }
}

/// Abstraction that allows for different wasi engines to be used.
/// The containerd shim setup by this library will use this trait to get an engine and pass that along to instances.
pub trait EngineGetter {
    type E: Send + Sync + Clone;
    fn new_engine() -> Result<Self::E, Error>;
}
