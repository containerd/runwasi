use std::cell::OnceCell;
use std::fs::File;
use std::io::Read;
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use libcontainer::workload::default::DefaultExecutor;
use libcontainer::workload::{
    Executor as LibcontainerExecutor, ExecutorError as LibcontainerExecutorError,
    ExecutorValidationError,
};
use oci_spec::runtime::Spec;

use crate::container::context::RuntimeContext;
use crate::container::engine::Engine;
use crate::sandbox::Stdio;

#[derive(Clone)]
enum InnerExecutor {
    Wasm,
    Linux { entrypoint: PathBuf },
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
            InnerExecutor::Linux { entrypoint } => {
                log::info!("executing linux container");
                let spec = replace_entrypoint(spec, entrypoint);
                self.stdio.take().redirect().unwrap();
                DefaultExecutor {}.exec(&spec)
            }
            InnerExecutor::Wasm => {
                log::info!("calling start function");
                match self.engine.run(spec, self.stdio.take()) {
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
            if let Ok(entrypoint) = is_linux_container(spec) {
                InnerExecutor::Linux { entrypoint }
            } else if self.engine.can_handle(spec).is_ok() {
                InnerExecutor::Wasm
            } else {
                InnerExecutor::CantHandle
            }
        })
    }
}

fn replace_entrypoint(spec: &Spec, entrypoint: impl AsRef<Path>) -> Spec {
    // libcontainer uses a slightly different logic to identify the entrypoing
    // based on the PATH env-var. To unequivocally identify the entrypoint,
    // replace it with an absolute path.
    let entrypoint = entrypoint.as_ref().to_string_lossy().to_string();
    let args = spec.args().iter().skip(1).cloned();
    let args = [entrypoint].into_iter().chain(args).collect::<Vec<_>>();
    let process = spec.process().as_ref().cloned().map(|mut process| {
        process.set_args(Some(args));
        process
    });
    let mut spec = spec.clone();
    spec.set_process(process);
    spec
}

fn is_linux_container(spec: &Spec) -> Result<PathBuf> {
    let executable = spec.entrypoint().context("no entrypoint provided")?;
    let executable = spec
        .find_in_path(executable)
        .find(|p| {
            p.metadata()
                .map(|m| m.permissions().mode())
                .is_ok_and(|mode| mode & 0o001 != 0)
        })
        .context("entrypoint not found")?;

    // check the shebang and ELF magic number
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format#File_header
    let mut buffer = [0; 4];
    File::open(&executable)?.read_exact(&mut buffer)?;

    match buffer {
        [0x7f, 0x45, 0x4c, 0x46] => Ok(executable), // ELF magic number
        [0x23, 0x21, ..] => Ok(executable),         // shebang
        _ => bail!("not a valid script or elf file"),
    }
}
