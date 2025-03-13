use std::cell::RefCell;
use std::io::Error as IoError;
use std::mem::transmute;

use anyhow::{Context, anyhow};
use libcontainer::container::Container as YoukiContainer;
use libcontainer::signal::Signal;
use serde::Serialize;
use serde::de::DeserializeOwned;
use shimkit::zygote::{WireError, Zygote};

thread_local! {
    // The youki's Container will live in a static inside the zygote process.
    // Reserve some space for it here.
    static CONTAINER: RefCell<Option<YoukiContainer>> = RefCell::default();
}

// The exposed container is just a wrapper around the zygore process
pub struct Container(Zygote);

// Constructor methods
impl Container {
    pub fn build<Arg: Serialize + DeserializeOwned + 'static>(
        f: fn(Arg) -> anyhow::Result<YoukiContainer>,
        arg: Arg,
    ) -> anyhow::Result<Self> {
        let zygote = Zygote::global().spawn();
        let container = Container(zygote);
        container.run_init(f, arg)?;

        Ok(container)
    }
}

// Wrap the youki's Container methods that we use
impl Container {
    pub fn pid(&self) -> anyhow::Result<i32> {
        self.run(|c, _| Ok(c.pid().map(|pid| pid.as_raw())), ())?
            .context("Failed to obtain PID")
    }

    pub fn start(&self) -> anyhow::Result<()> {
        self.run(|c, _| Ok(c.start()?), ())
    }
    pub fn kill(&self, signal: u32) -> anyhow::Result<()> {
        self.run(
            |c, signal| {
                let signal = Signal::try_from(signal as i32).context("invalid signal number")?;
                Ok(c.kill(signal, true)?)
            },
            signal,
        )
    }
    pub fn delete(&self) -> anyhow::Result<()> {
        self.run(|c, _| Ok(c.delete(true)?), ())
    }
}

impl Container {
    fn run_impl<
        Arg: Serialize + DeserializeOwned + 'static,
        T: Serialize + DeserializeOwned + 'static,
    >(
        &self,
        f: fn(&mut Option<YoukiContainer>, Arg) -> anyhow::Result<T>,
        arg: Arg,
    ) -> anyhow::Result<T> {
        self.0
            .run(
                |(f, arg)| {
                    let f: fn(&mut Option<YoukiContainer>, Arg) -> anyhow::Result<T> =
                        unsafe { transmute(f) };
                    CONTAINER.with_borrow_mut(|c| -> Result<T, WireError> {
                        Ok(f(c, arg).map_err(IoError::other)?)
                    })
                },
                (f as usize, arg),
            )
            .map_err(|e| anyhow!(e))
    }

    fn run_init<Arg: Serialize + DeserializeOwned + 'static>(
        &self,
        f: fn(Arg) -> anyhow::Result<YoukiContainer>,
        arg: Arg,
    ) -> anyhow::Result<()> {
        self.run_impl(
            |c: &mut Option<YoukiContainer>, (f, arg): (usize, Arg)| -> anyhow::Result<()> {
                let f: fn(Arg) -> anyhow::Result<YoukiContainer> = unsafe { transmute(f) };
                *c = Some(f(arg)?);
                Ok(())
            },
            (f as usize, arg),
        )
    }

    fn run<
        Arg: Serialize + DeserializeOwned + 'static,
        T: Serialize + DeserializeOwned + 'static,
    >(
        &self,
        f: fn(&mut YoukiContainer, Arg) -> anyhow::Result<T>,
        arg: Arg,
    ) -> anyhow::Result<T> {
        self.run_impl(
            |c: &mut Option<YoukiContainer>, (f, arg): (usize, Arg)| -> anyhow::Result<T> {
                let f: fn(&mut YoukiContainer, Arg) -> anyhow::Result<T> = unsafe { transmute(f) };
                let c = c.as_mut().expect("Container not initialized");
                f(c, arg)
            },
            (f as usize, arg),
        )
    }
}
