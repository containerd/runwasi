use std::path::{Path, PathBuf};

use oci_spec::runtime::Spec;

use crate::sandbox::oci::OciArtifact;

pub trait RuntimeContext {
    // ctx.args() returns arguments from the runtime spec process field, including the
    // path to the entrypoint executable.
    fn args(&self) -> &[String];

    // ctx.entrypoint() returns the entrypoint path from arguments on the runtime
    // spec process field.
    fn entrypoint(&self) -> Option<&Path>;

    // ctx.wasi_entrypoint() returns a `WasiEntrypoint` with the path to the module to use
    // as an entrypoint and the name of the exported function to call, obtained from the
    // arguments on process OCI spec.
    // The girst argument in the spec is specified as `path#func` where `func` is optional
    // and defaults to _start, e.g.:
    //   "/app/app.wasm#entry" -> { path: "/app/app.wasm", func: "entry" }
    //   "my_module.wat" -> { path: "my_module.wat", func: "_start" }
    //   "#init" -> { path: "", func: "init" }
    fn wasi_entrypoint(&self) -> WasiEntrypoint;

    fn oci_artifacts(&self) -> &[OciArtifact];
}

pub struct WasiEntrypoint {
    pub path: PathBuf,
    pub func: String,
}

pub(crate) struct WasiContext<'a> {
    pub spec: &'a Spec,
    pub oci_artifacts: &'a [OciArtifact],
}

impl RuntimeContext for WasiContext<'_> {
    fn args(&self) -> &[String] {
        self.spec
            .process()
            .as_ref()
            .and_then(|p| p.args().as_ref())
            .map(|a| a.as_slice())
            .unwrap_or_default()
    }

    fn entrypoint(&self) -> Option<&Path> {
        self.args().first().map(Path::new)
    }

    fn wasi_entrypoint(&self) -> WasiEntrypoint {
        let arg0 = self.args().first().map(String::as_str).unwrap_or("");
        let (path, func) = arg0.split_once('#').unwrap_or((arg0, "_start"));
        WasiEntrypoint {
            path: PathBuf::from(path),
            func: func.to_string(),
        }
    }

    fn oci_artifacts(&self) -> &[OciArtifact] {
        self.oci_artifacts
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};

    use super::*;

    #[test]
    fn test_get_args() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec!["hello.wat".to_string()])
                    .build()?,
            )
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            oci_artifacts: &[],
        };

        let args = ctx.args();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], "hello.wat");

        Ok(())
    }

    #[test]
    fn test_get_args_return_empty() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(ProcessBuilder::default().cwd("/").args(vec![]).build()?)
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            oci_artifacts: &[],
        };

        let args = ctx.args();
        assert_eq!(args.len(), 0);

        Ok(())
    }

    #[test]
    fn test_get_args_returns_all() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec![
                        "hello.wat".to_string(),
                        "echo".to_string(),
                        "hello".to_string(),
                    ])
                    .build()?,
            )
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            oci_artifacts: &[],
        };

        let args = ctx.args();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "hello.wat");
        assert_eq!(args[1], "echo");
        assert_eq!(args[2], "hello");

        Ok(())
    }

    #[test]
    fn test_get_module_returns_none_when_not_present() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(ProcessBuilder::default().cwd("/").args(vec![]).build()?)
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            oci_artifacts: &[],
        };

        let path = ctx.wasi_entrypoint().path;
        assert!(path.as_os_str().is_empty());

        Ok(())
    }

    #[test]
    fn test_get_module_returns_function() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec![
                        "hello.wat#foo".to_string(),
                        "echo".to_string(),
                        "hello".to_string(),
                    ])
                    .build()?,
            )
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            oci_artifacts: &[],
        };

        let WasiEntrypoint { path, func } = ctx.wasi_entrypoint();
        assert_eq!(path, Path::new("hello.wat"));
        assert_eq!(func, "foo");

        Ok(())
    }

    #[test]
    fn test_get_module_returns_start() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec![
                        "/root/hello.wat".to_string(),
                        "echo".to_string(),
                        "hello".to_string(),
                    ])
                    .build()?,
            )
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            oci_artifacts: &[],
        };

        let WasiEntrypoint { path, func } = ctx.wasi_entrypoint();
        assert_eq!(path, Path::new("/root/hello.wat"));
        assert_eq!(func, "_start");

        Ok(())
    }
}
