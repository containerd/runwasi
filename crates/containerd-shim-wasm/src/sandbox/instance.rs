use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use chrono::{DateTime, Utc};

use super::error::Error;

type ExitCode = (Mutex<Option<(u32, DateTime<Utc>)>>, Condvar);

#[derive(Clone)]
pub struct InstanceConfig<E>
where
    E: Send + Sync + Clone,
{
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

// This is used for the "pause" container with cri.
pub struct Nop {
    // Since we are faking the container, we need to keep track of the "exit" code/time
    // We'll just mark it as exited when kill is called.
    exit_code: Arc<ExitCode>,
}

impl Instance for Nop {
    type E = ();
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
    use std::sync::mpsc::channel;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

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
