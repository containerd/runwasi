use std::cell::OnceCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::prelude::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use containerd_shimkit::AmbientRuntime;
use libcontainer::workload::default::DefaultExecutor;
use libcontainer::workload::{
    Executor as LibcontainerExecutor, ExecutorError as LibcontainerExecutorError,
    ExecutorSetEnvsError, ExecutorValidationError,
};
use oci_spec::runtime::Spec;

use crate::sandbox::Sandbox;
use crate::sandbox::context::{RuntimeContext, Source, WasiContext, WasmLayer};
use crate::sandbox::path::PathResolve;
use crate::shim::Shim;

#[derive(Clone)]
enum ExecutorType<S: Shim> {
    Wasm(S::Sandbox),
    Linux,
    CantHandle,
}

pub(crate) struct Executor<S: Shim>(Arc<InnerExecutor<S>>);

impl<S: Shim> Clone for Executor<S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

pub(crate) struct InnerExecutor<S: Shim> {
    ty: OnceCell<ExecutorType<S>>,
    wasm_layers: Vec<WasmLayer>,
}

impl<S: Shim> LibcontainerExecutor for Executor<S> {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Debug"))]
    fn validate(&self, spec: &Spec) -> Result<(), ExecutorValidationError> {
        // We can handle linux container. We delegate wasm container to the engine.
        match self.ty(spec) {
            ExecutorType::CantHandle => Err(ExecutorValidationError::CantHandle(S::name())),
            _ => Ok(()),
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "Debug"))]
    fn exec(&self, spec: &Spec) -> Result<(), LibcontainerExecutorError> {
        // If it looks like a linux container, run it as a linux container.
        // Otherwise, run it as a wasm container
        match self.ty(spec) {
            ExecutorType::CantHandle => Err(LibcontainerExecutorError::CantHandle(S::name())),
            ExecutorType::Linux => {
                log::info!("executing linux container");
                DefaultExecutor {}.exec(spec)
            }
            ExecutorType::Wasm(container) => {
                let ctx = self.ctx(spec);
                log::info!("calling start function");
                match container.run_wasi(&ctx).block_on() {
                    Ok(code) => std::process::exit(code),
                    Err(err) => {
                        log::info!("error running start function: {err}");
                        std::process::exit(137)
                    }
                };
            }
        }
    }

    // For Wasm containers this is a no-op: instead of youki's libcontainer setting the envs in the
    // shim process, the shim manages the envs itself. The expectation is that the shim will call
    // `RuntimeContext::envs()` to get the container's envs and set them in the `Engine::run_wasi`
    // function. This way, the shim can decide how to pass the envs to the WASI context.
    //
    // Linux (native) containers have no such shim code, so for them we fall back to libcontainer's
    // default behaviour (clear the host's envs, then set the ones from the OCI spec). Without this a
    // native container sharing a pod with a Wasm container (e.g. Knative's queue-proxy) runs with
    // the shim's own environment, including its PATH.
    //
    // libcontainer calls `validate` right before `setup_envs`, so the container type is known here.
    //
    // See the following issues for more context:
    // https://github.com/containerd/runwasi/issues/619
    // https://github.com/containerd/runwasi/issues/1061
    // https://github.com/containers/youki/issues/2815
    fn setup_envs(
        &self,
        envs: HashMap<String, String>,
    ) -> std::result::Result<(), ExecutorSetEnvsError> {
        match self.0.ty.get() {
            Some(ExecutorType::Linux) => DefaultExecutor {}.setup_envs(envs),
            Some(ExecutorType::Wasm(_) | ExecutorType::CantHandle) => Ok(()),
            None => Err(ExecutorSetEnvsError::Other(
                "setup_envs called before validate: unknown container type".to_string(),
            )),
        }
    }
}

impl<S: Shim> Executor<S> {
    pub fn new(wasm_layers: Vec<WasmLayer>) -> Self {
        Self(Arc::new(InnerExecutor {
            ty: Default::default(),
            wasm_layers,
        }))
    }

    fn ctx<'a>(&'a self, spec: &'a Spec) -> WasiContext<'a> {
        let wasm_layers = &self.0.wasm_layers;
        WasiContext { spec, wasm_layers }
    }

    fn ty(&self, spec: &Spec) -> &ExecutorType<S> {
        self.0.ty.get_or_init(|| {
            let ctx = &self.ctx(spec);
            match is_linux_container(ctx) {
                Ok(_) => ExecutorType::Linux,
                Err(err) => {
                    log::debug!("error checking if linux container: {err}. Fallback to wasm container");
                    let container = S::Sandbox::default();
                    match container.can_handle(ctx).block_on() {
                        Ok(_) => ExecutorType::Wasm(container),
                        Err(err) => {
                            // log an error and return
                            log::error!("error checking if wasm container: {err}. Note: arg0 must be a path to a Wasm file");
                            ExecutorType::CantHandle
                        }
                    }
                }
            }
        })
    }
}

fn is_linux_container(ctx: &impl RuntimeContext) -> Result<()> {
    if let Source::Oci(_) = ctx.entrypoint().source {
        bail!("the entry point contains wasm layers")
    };

    let executable = ctx
        .entrypoint()
        .arg0
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};

    use super::*;
    use crate::sandbox::Sandbox;
    use crate::sandbox::context::RuntimeContext;

    #[derive(Default)]
    struct TestSandbox;

    impl Sandbox for TestSandbox {
        async fn can_handle(&self, _ctx: &impl RuntimeContext) -> Result<()> {
            Ok(())
        }
        async fn run_wasi(&self, _ctx: &impl RuntimeContext) -> Result<i32> {
            Ok(0)
        }
    }

    struct TestShim;

    impl Shim for TestShim {
        fn name() -> &'static str {
            "test"
        }
        type Sandbox = TestSandbox;
    }

    fn spec(args: &[&str], env: &[&str]) -> Spec {
        SpecBuilder::default()
            .root(RootBuilder::default().path("/").build().unwrap())
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                    .env(env.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap()
    }

    fn envs(spec: &Spec) -> HashMap<String, String> {
        spec.process()
            .as_ref()
            .and_then(|p| p.env().as_ref())
            .into_iter()
            .flatten()
            .filter_map(|e| e.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const HELPER_ENV: &str = "RUNWASI_TEST_EXEC_HELPER";

    /// Runs the same sequence libcontainer's init process runs (`validate`, `setup_envs`, `exec`)
    /// in this process. It replaces the process on success, so it only does something as the
    /// helper child of `run_like_libcontainer`; as a normal test it returns immediately.
    #[test]
    fn exec_helper() {
        let Ok(program) = std::env::var(HELPER_ENV) else {
            return;
        };
        let spec = spec(
            &[program.as_str(), "env"],
            &["FOO=bar", "CONTAINER_CONCURRENCY=0", "PATH=/container/bin"],
        );
        let executor = Executor::<TestShim>::new(vec![]);
        executor.validate(&spec).unwrap();
        executor.setup_envs(envs(&spec)).unwrap();
        let err = executor.exec(&spec).unwrap_err();
        panic!("exec returned: {err}");
    }

    /// Re-runs this test binary as a clean child (no fork in a multithreaded process) with a
    /// polluted "shim" environment, and returns what the exec'd program printed.
    fn run_like_libcontainer(program: &str) -> String {
        let test_name = format!(
            "{}::exec_helper",
            module_path!().split_once("::").unwrap().1
        );
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                test_name.as_str(),
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(HELPER_ENV, program)
            .env("SHIM_ONLY_VAR", "leaked")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    // Reproduces https://github.com/containerd/runwasi/issues/1061 with a static busybox:
    // `env` must print the container's env and nothing from the shim's environment.
    #[test]
    fn linux_container_gets_its_own_env() {
        let Some(busybox) = ["/usr/bin/busybox", "/bin/busybox"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
        else {
            eprintln!("busybox not installed, skipping");
            return;
        };
        let out = run_like_libcontainer(busybox);
        // (the first line shares a line with the test harness's "test ... " prefix)
        assert!(out.contains("FOO=bar"), "{out}");
        assert!(out.contains("CONTAINER_CONCURRENCY=0"), "{out}");
        assert!(out.contains("PATH=/container/bin"), "{out}");
        assert!(
            !out.contains("SHIM_ONLY_VAR"),
            "shim env leaked into container: {out}"
        );
    }

    #[test]
    fn setup_envs_before_validate_is_an_error() {
        let executor = Executor::<TestShim>::new(vec![]);
        assert!(executor.setup_envs(HashMap::new()).is_err());
    }

    #[test]
    fn wasm_container_envs_are_left_to_the_shim() {
        // arg0 is not an ELF/script, so the executor treats it as a Wasm container.
        let spec = spec(&["/hello.wasm"], &["FOO=bar"]);
        let executor = Executor::<TestShim>::new(vec![]);
        executor.validate(&spec).unwrap();
        let before: HashMap<_, _> = std::env::vars().collect();
        executor.setup_envs(envs(&spec)).unwrap();
        let after: HashMap<_, _> = std::env::vars().collect();
        assert_eq!(
            before, after,
            "setup_envs must not touch the process env for Wasm"
        );
    }
}
