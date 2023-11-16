use std::path::{Path, PathBuf};

use oci_spec::image::Platform;
use oci_spec::runtime::Spec;

use crate::sandbox::oci::WasmLayer;

pub trait RuntimeContext {
    // ctx.args() returns arguments from the runtime spec process field, including the
    // path to the entrypoint executable.
    fn args(&self) -> &[String];

    // ctx.wasi_entrypoint() returns a `WasiEntrypoint` with the path to the module to use
    // as an entrypoint and the name of the exported function to call, obtained from the
    // arguments on process OCI spec.
    // The girst argument in the spec is specified as `path#func` where `func` is optional
    // and defaults to _start, e.g.:
    //   "/app/app.wasm#entry" -> { path: "/app/app.wasm", func: "entry" }
    //   "my_module.wat" -> { path: "my_module.wat", func: "_start" }
    //   "#init" -> { path: "", func: "init" }
    fn entrypoint(&self) -> WasiEntrypoint;

    fn wasi_loading_strategy(&self) -> WasiLoadingStrategy;

    fn platform(&self) -> &Platform;
}

pub enum WasiLoadingStrategy<'a> {
    File(PathBuf),
    Oci(&'a [WasmLayer]),
}

pub struct WasiEntrypoint<'a> {
    pub path: PathBuf,
    pub func: String,
    pub arg0: Option<&'a Path>,
}

pub(crate) struct WasiContext<'a> {
    pub spec: &'a Spec,
    pub wasm_layers: &'a [WasmLayer],
    pub platform: &'a Platform,
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

    fn entrypoint(&self) -> WasiEntrypoint {
        let arg0 = self.args().first();

        let entry_point = arg0.map(String::as_str).unwrap_or("");
        let (path, func) = entry_point
            .split_once('#')
            .unwrap_or((entry_point, "_start"));
        WasiEntrypoint {
            path: PathBuf::from(path),
            func: func.to_string(),
            arg0: arg0.map(Path::new),
        }
    }

    fn wasi_loading_strategy(&self) -> WasiLoadingStrategy {
        if self.wasm_layers.is_empty() {
            WasiLoadingStrategy::File(self.entrypoint().path.clone())
        } else {
            WasiLoadingStrategy::Oci(self.wasm_layers)
        }
    }

    fn platform(&self) -> &Platform {
        self.platform
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use oci_spec::image::Descriptor;
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
            wasm_layers: &[],
            platform: &Platform::default(),
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
            wasm_layers: &[],
            platform: &Platform::default(),
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
            wasm_layers: &[],
            platform: &Platform::default(),
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
            wasm_layers: &[],
            platform: &Platform::default(),
        };

        let path = ctx.entrypoint().path;
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
            wasm_layers: &[],
            platform: &Platform::default(),
        };

        let WasiEntrypoint { path, func, arg0 } = ctx.entrypoint();
        assert_eq!(path, Path::new("hello.wat"));
        assert_eq!(func, "foo");
        assert_eq!(arg0, Some(Path::new("hello.wat#foo")));

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
            wasm_layers: &[],
            platform: &Platform::default(),
        };

        let WasiEntrypoint { path, func, arg0 } = ctx.entrypoint();
        assert_eq!(path, Path::new("/root/hello.wat"));
        assert_eq!(func, "_start");
        assert_eq!(arg0, Some(Path::new("/root/hello.wat")));

        Ok(())
    }

    #[test]
    fn test_loading_strategy_is_file_when_no_layers() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args(vec![
                        "/root/hello.wat#foo".to_string(),
                        "echo".to_string(),
                        "hello".to_string(),
                    ])
                    .build()?,
            )
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            wasm_layers: &[],
            platform: &Platform::default(),
        };

        let expected_path = PathBuf::from("/root/hello.wat");
        assert!(matches!(
            ctx.wasi_loading_strategy(),
            WasiLoadingStrategy::File(p) if p == expected_path
        ));

        Ok(())
    }

    #[test]
    fn test_loading_strategy_is_oci_when_layers_present() -> Result<()> {
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
            wasm_layers: &[WasmLayer {
                layer: vec![],
                config: Descriptor::new(oci_spec::image::MediaType::Other("".to_string()), 10, ""),
            }],
            platform: &Platform::default(),
        };

        assert!(matches!(
            ctx.wasi_loading_strategy(),
            WasiLoadingStrategy::Oci(_)
        ));

        Ok(())
    }
}
