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
pub struct InstanceConfig<E: Engine> {
    /// The WASI engine to use.
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
    /// Namespace for containerd
    namespace: String,
    // /// GRPC address back to main containerd
    containerd_address: String,
}

impl<E: Engine> InstanceConfig<E> {
    pub fn new(engine: E, namespace: String, containerd_address: String) -> Self {
        Self {
            engine,
            namespace,
            stdin: None,
            stdout: None,
            stderr: None,
            bundle: None,
            containerd_address,
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

    /// get the namespace for the instance
    pub fn get_namespace(&self) -> String {
        self.namespace.clone()
    }

    /// get the containerd address for the instance
    pub fn get_containerd_address(&self) -> String {
        self.containerd_address.clone()
    }
}

/// Represents a WASI module(s).
/// Instance is a trait that gets implemented by consumers of this library.
pub trait Instance<E: Engine> {
    /// Create a new instance
    fn new(id: String, cfg: Option<&InstanceConfig<E>>) -> Self;

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, Error>;

    /// Send a signal to the instance
    fn kill(&self, signal: u32) -> Result<(), Error>;

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), Error>;

    /// Set up waiting for the instance to exit
    /// The Wait struct is used to send the exit code and time back to the
    /// caller. The recipient is expected to call function
    /// set_up_exit_code_wait() implemented by Wait to set up exit code
    /// processing. Note that the "wait" function doesn't block, but
    /// it sets up the waiting channel.
    fn wait(&self, waiter: &Wait) -> Result<(), Error>;
}

pub trait Engine: Clone + Send + Sync {
    fn new() -> Self;
}

/// This is used for waiting for the container process to exit and deliver the exit code to the caller.
/// Since the shim needs to provide the caller the process exit code, this struct wraps the required
/// thread setup to make the shims simpler.
pub struct Wait {
    tx: Sender<(u32, DateTime<Utc>)>,
}

impl Wait {
    /// Create a new Wait struct with the provided sending endpoint of a channel.
    pub fn new(sender: Sender<(u32, DateTime<Utc>)>) -> Self {
        Wait { tx: sender }
    }

    /// This is called by the shim to create the thread to wait for the exit
    /// code. When the child process exits, the shim will use the ExitCode
    /// to signal the exit status to the caller. This function returns so that
    /// the wait() function in the shim implementation API would not block.
    pub fn set_up_exit_code_wait(&self, exit_code: Arc<ExitCode>) -> Result<(), Error> {
        let sender = self.tx.clone();
        let code = Arc::clone(&exit_code);
        thread::spawn(move || {
            let (lock, cvar) = &*code;
            let mut exit = lock.lock().unwrap();
            while (*exit).is_none() {
                exit = cvar.wait(exit).unwrap();
            }
            let ec = (*exit).unwrap();
            sender.send(ec).unwrap();
        });

        Ok(())
    }
}

/// This is used for the "pause" container with cri and is a no-op instance implementation.
pub struct Nop {
    /// Since we are faking the container, we need to keep track of the "exit" code/time
    /// We'll just mark it as exited when kill is called.
    exit_code: Arc<ExitCode>,
}

impl Engine for () {
    fn new() -> Self {}
}

impl Instance<()> for Nop {
    fn new(_id: String, _cfg: Option<&InstanceConfig<()>>) -> Self {
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
    fn wait(&self, waiter: &Wait) -> Result<(), Error> {
        waiter.set_up_exit_code_wait(self.exit_code.clone())
    }
}

#[cfg(test)]
mod noptests {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use libc::SIGHUP;

    use super::*;

    #[test]
    fn test_nop_kill_sigkill() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None);
        let (tx, rx) = channel();
        let waiter = Wait::new(tx);

        nop.wait(&waiter).unwrap();
        nop.kill(SIGKILL as u32)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 137);
        Ok(())
    }

    #[test]
    fn test_nop_kill_sigterm() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None);
        let (tx, rx) = channel();
        let waiter = Wait::new(tx);

        nop.wait(&waiter).unwrap();
        nop.kill(SIGTERM as u32)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 0);
        Ok(())
    }

    #[test]
    fn test_nop_kill_sigint() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None);
        let (tx, rx) = channel();
        let waiter = Wait::new(tx);

        nop.wait(&waiter).unwrap();
        nop.kill(SIGINT as u32)?;
        let ec = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(ec.0, 0);
        Ok(())
    }

    #[test]
    fn test_op_kill_other() -> Result<(), Error> {
        let nop = Nop::new("".to_string(), None);

        let err = nop.kill(SIGHUP as u32).unwrap_err();
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
