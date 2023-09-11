use std::cell::OnceCell;
use std::fs::File;
use std::io::Read;
use std::os::unix::prelude::PermissionsExt;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use libcontainer::workload::default::DefaultExecutor;
use libcontainer::workload::{
    Executor as LibcontainerExecutor, ExecutorError as LibcontainerExecutorError,
    ExecutorValidationError,
};
use oci_spec::runtime::Spec;

use crate::container::context::RuntimeContext;
use crate::container::engine::Engine;
use crate::container::PathResolve;
use crate::sandbox::Stdio;

#[derive(Clone)]
enum InnerExecutor {
    Wasm,
    Linux,
    CantHandle,
}

#[derive(Clone)]
pub(crate) struct Executor<E: Engine> {
    engine: E,
    stdio: Stdio,
    inner: OnceCell<InnerExecutor>,
}

impl<E: Engine> LibcontainerExecutor for Executor<E> {
    fn validate(&self, spec: &Spec) -> Result<(), ExecutorValidationError> {
        // We can handle linux container. We delegate wasm container to the engine.
        match self.inner(spec) {
            InnerExecutor::CantHandle => Err(ExecutorValidationError::CantHandle(E::name())),
            _ => Ok(()),
        }
    }

    fn exec(&self, spec: &Spec) -> Result<(), LibcontainerExecutorError> {
        // If it looks like a linux container, run it as a linux container.
        // Otherwise, run it as a wasm container
        match self.inner(spec) {
            InnerExecutor::CantHandle => Err(LibcontainerExecutorError::CantHandle(E::name())),
            InnerExecutor::Linux => {
                log::info!("executing linux container");
                self.stdio.take().redirect().unwrap();
                DefaultExecutor {}.exec(spec)
            }
            InnerExecutor::Wasm => {
                log::info!("calling start function");
                match self.engine.run_wasi(spec, self.stdio.take()) {
                    Ok(code) => std::process::exit(code),
                    Err(err) => {
                        log::info!("error running start function: {err}");
                        std::process::exit(137)
                    }
                };
            }
        }
    }
}

impl<E: Engine> Executor<E> {
    pub fn new(engine: E, stdio: Stdio) -> Self {
        Self {
            engine,
            stdio,
            inner: Default::default(),
        }
    }

    fn inner(&self, spec: &Spec) -> &InnerExecutor {
        self.inner.get_or_init(|| {
            if is_linux_container(spec).is_ok() {
                InnerExecutor::Linux
            } else if self.engine.can_handle(spec).is_ok() {
                InnerExecutor::Wasm
            } else {
                InnerExecutor::CantHandle
            }
        })
    }
}

fn is_linux_container(spec: &Spec) -> Result<()> {
    let executable = spec
        .entrypoint()
        .context("no entrypoint provided")?
        .resolve_in_path()
        .find_map(|p| -> Option<PathBuf> {
            let mode = p.metadata().ok()?.permissions().mode();
            (mode & 0o001 != 0).then_some(p)
        })
        .context("entrypoint not found")?;

    // check the shebang and ELF magic number
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format#File_header
    let mut buffer = [0; 4];
    File::open(executable)?.read_exact(&mut buffer)?;

    match buffer {
        [0x7f, 0x45, 0x4c, 0x46] => Ok(()), // ELF magic number
        [0x23, 0x21, ..] => Ok(()),         // shebang
        _ => bail!("not a valid script or elf file"),
    }
}
