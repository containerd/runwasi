use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oci_spec::runtime::Spec;

use crate::container::PathResolve;

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

    fn resolved_wasi_entrypoint(&self) -> Result<WasiEntrypoint>;
}

pub struct WasiEntrypoint {
    pub path: PathBuf,
    pub func: String,
}

impl RuntimeContext for Spec {
    fn args(&self) -> &[String] {
        self.process()
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

    fn resolved_wasi_entrypoint(&self) -> Result<WasiEntrypoint> {
        let wasi_entrypoint = self.wasi_entrypoint();

        let path = wasi_entrypoint
            .path
            .resolve_in_path_or_cwd()
            .next()
            .context("module not found")?;

        let mut buffer = [0; 4];
        File::open(&path)?.read_exact(&mut buffer)?;

        if buffer.as_slice() != b"\0asm" {
            // Check if this is a `.wat` file
            wat::parse_file(&path)?;
        }

        Ok(WasiEntrypoint {
            path,
            ..wasi_entrypoint
        })
    }
}

impl From<WasiEntrypoint> for (PathBuf, String) {
    fn from(value: WasiEntrypoint) -> Self {
        (value.path, value.func)
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
        let spec = &spec;

        let args = spec.args();
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
        let spec = &spec;

        let args = spec.args();
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
        let spec = &spec;

        let args = spec.args();
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
        let spec = &spec;

        let path = spec.wasi_entrypoint().path;
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
        let spec = &spec;

        let WasiEntrypoint { path, func } = spec.wasi_entrypoint();
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
        let spec = &spec;

        let WasiEntrypoint { path, func } = spec.wasi_entrypoint();
        assert_eq!(path, Path::new("/root/hello.wat"));
        assert_eq!(func, "_start");

        Ok(())
    }
}
