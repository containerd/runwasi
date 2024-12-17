use std::borrow::Cow;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use oci_spec::image::Platform;
use oci_spec::runtime::Spec;

use crate::container::path::PathResolve;
use crate::sandbox::oci::WasmLayer;

pub trait RuntimeContext {
    // ctx.args() returns arguments from the runtime spec process field, including the
    // path to the entrypoint executable.
    fn args(&self) -> &[String];

    // ctx.envs() returns environment variables in the format `ENV_VAR_NAME=VALUE` from the runtime spec process field.
    fn envs(&self) -> &[String];

    // ctx.entrypoint() returns a `Entrypoint` with the following fields obtained from the first argument in the OCI spec for entrypoint:
    //   - `arg0` - raw entrypoint from the OCI spec
    //   - `name` - provided as the file name of the module in the entrypoint without the extension
    //   - `func` - name of the exported function to call, obtained from the
    // arguments on process OCI spec.
    //  - `Source` - either a `File(PathBuf)` or `Oci(WasmLayer)`. When a `File` source the `PathBuf`` is provided by entrypoint in OCI spec.
    //     If the image contains custom OCI Wasm layers, the source is provided as an array of `WasmLayer` structs.
    //
    // The first argument in the OCI spec for entrypoint is specified as `path#func` where `func` is optional
    // and defaults to _start, e.g.:
    //   "/app/app.wasm#entry" -> { source: File("/app/app.wasm"), func: "entry", name: "Some(app)", arg0: "/app/app.wasm#entry" }
    //   "my_module.wat" -> { source: File("my_module.wat"), func: "_start", name: "Some(my_module)", arg0: "my_module.wat" }
    //   "#init" -> { source: File(""), func: "init", name: None, arg0: "#init" }
    fn entrypoint(&self) -> Entrypoint;

    // the platform for the container using the struct defined on the OCI spec definition
    // https://github.com/opencontainers/image-spec/blob/v1.1.0-rc5/image-index.md
    fn platform(&self) -> &Platform;
}

/// The source for a WASI module / components.
#[derive(Debug)]
pub enum Source<'a> {
    // The WASI module is a file in the file system.
    File(PathBuf),
    // The WASI module / component is provided as a layer in the OCI spec.
    // For a WASI preview 1 module this is usually a single element array.
    // For a WASI preview 2 component this is an array of one or more
    // elements, where each element is a component.
    // Runtimes can additionally provide a list of layer types they support,
    // and they will be included in this array, e.g., a `toml` file with the
    // runtime configuration.
    Oci(&'a [WasmLayer]),
}

impl<'a> Source<'a> {
    pub fn as_bytes(&self) -> anyhow::Result<Cow<'a, [u8]>> {
        match self {
            Source::File(path) => {
                let path = path
                    .resolve_in_path_or_cwd()
                    .next()
                    .context("module not found")?;
                Ok(Cow::Owned(std::fs::read(path)?))
            }
            Source::Oci([module]) => Ok(Cow::Borrowed(&module.layer)),
            Source::Oci(_modules) => {
                bail!("only a single module is supported when using images with OCI layers")
            }
        }
    }
}

/// The entrypoint for a WASI module / component.
///
pub struct Entrypoint<'a> {
    pub func: String,
    pub name: Option<String>,
    pub arg0: Option<&'a Path>,
    pub source: Source<'a>,
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

    fn envs(&self) -> &[String] {
        self.spec
            .process()
            .as_ref()
            .and_then(|p| p.env().as_ref())
            .map(|e| e.as_slice())
            .unwrap_or_default()
    }

    fn entrypoint(&self) -> Entrypoint {
        let arg0 = self.args().first();

        let entry_point = arg0.map(String::as_str).unwrap_or("");
        let (path, func) = entry_point
            .split_once('#')
            .unwrap_or((entry_point, "_start"));

        let source = if self.wasm_layers.is_empty() {
            Source::File(PathBuf::from(path))
        } else {
            Source::Oci(self.wasm_layers)
        };

        let module_name = PathBuf::from(path)
            .file_stem()
            .map(|name| name.to_string_lossy().to_string());

        Entrypoint {
            func: func.to_string(),
            arg0: arg0.map(Path::new),
            source,
            name: module_name,
        }
    }

    fn platform(&self) -> &Platform {
        self.platform
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use oci_spec::image::{Descriptor, Digest};
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

        let path = ctx.entrypoint().source;
        assert!(matches!(
            path,
            Source::File(p) if p.as_os_str().is_empty()
        ));

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

        let expected_path = PathBuf::from("hello.wat");
        let Entrypoint {
            name,
            func,
            arg0,
            source,
        } = ctx.entrypoint();
        assert_eq!(name, Some("hello".to_string()));
        assert_eq!(func, "foo");
        assert_eq!(arg0, Some(Path::new("hello.wat#foo")));
        assert!(matches!(
            source,
            Source::File(p) if p == expected_path
        ));

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

        let expected_path = PathBuf::from("/root/hello.wat");
        let Entrypoint {
            name,
            func,
            arg0,
            source,
        } = ctx.entrypoint();
        assert_eq!(name, Some("hello".to_string()));
        assert_eq!(func, "_start");
        assert_eq!(arg0, Some(Path::new("/root/hello.wat")));
        assert!(matches!(
            source,
            Source::File(p) if p == expected_path
        ));

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
            ctx.entrypoint().source,
            Source::File(p) if p == expected_path
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
                config: Descriptor::new(
                    oci_spec::image::MediaType::Other("".to_string()),
                    10,
                    Digest::try_from(format!("sha256:{:064?}", 0))?,
                ),
            }],
            platform: &Platform::default(),
        };

        assert!(matches!(ctx.entrypoint().source, Source::Oci(_)));

        Ok(())
    }

    #[test]
    fn test_get_envs() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .env(vec!["KEY1=VALUE1".to_string(), "KEY2=VALUE2".to_string()])
                    .build()?,
            )
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            wasm_layers: &[],
            platform: &Platform::default(),
        };

        let envs = ctx.envs();
        assert_eq!(envs.len(), 2);
        assert_eq!(envs[0], "KEY1=VALUE1");
        assert_eq!(envs[1], "KEY2=VALUE2");

        Ok(())
    }

    #[test]
    fn test_get_envs_return_empty() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(ProcessBuilder::default().cwd("/").env(vec![]).build()?)
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            wasm_layers: &[],
            platform: &Platform::default(),
        };

        let envs = ctx.envs();
        assert_eq!(envs.len(), 0);

        Ok(())
    }

    #[test]
    fn test_envs_return_default_only() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(ProcessBuilder::default().cwd("/").build()?)
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            wasm_layers: &[],
            platform: &Platform::default(),
        };

        let envs = ctx.envs();
        assert_eq!(envs.len(), 2);

        Ok(())
    }
}
