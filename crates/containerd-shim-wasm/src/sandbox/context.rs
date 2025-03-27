use std::borrow::Cow;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use oci_spec::image::{Descriptor, Platform};
use oci_spec::runtime::Spec;
use serde::{Deserialize, Serialize};
use wasmparser::Parser;

use crate::sandbox::path::PathResolve;

/// The `RuntimeContext` trait provides access to the runtime context that includes
/// the arguments, environment variables, and entrypoint for the container.
pub trait RuntimeContext: Send + Sync {
    /// Returns arguments from the runtime spec process field, including the
    /// path to the entrypoint executable.
    fn args(&self) -> &[String];

    /// Returns environment variables in the format `ENV_VAR_NAME=VALUE` from the runtime spec process field.
    fn envs(&self) -> &[String];

    /// Returns a `Entrypoint` with the following fields obtained from the first argument in the OCI spec for entrypoint:
    ///   - `arg0` - raw entrypoint from the OCI spec
    ///   - `name` - provided as the file name of the module in the entrypoint without the extension
    ///   - `func` - name of the exported function to call, obtained from the arguments on process OCI spec.
    ///   - `Source` - either a `File(PathBuf)` or `Oci(WasmLayer)`. When a `File` source the `PathBuf`` is provided by entrypoint in OCI spec.
    ///     If the image contains custom OCI Wasm layers, the source is provided as an array of `WasmLayer` structs.
    ///
    /// The first argument in the OCI spec for entrypoint is specified as `path#func` where `func` is optional
    /// and defaults to _start, e.g.:
    ///   "/app/app.wasm#entry" -> { source: File("/app/app.wasm"), func: "entry", name: "Some(app)", arg0: "/app/app.wasm#entry" }
    ///   "my_module.wat" -> { source: File("my_module.wat"), func: "_start", name: "Some(my_module)", arg0: "my_module.wat" }
    ///   "#init" -> { source: File(""), func: "init", name: None, arg0: "#init" }
    fn entrypoint(&self) -> Entrypoint;

    /// Returns the platform for the container using the struct defined on the OCI spec definition
    /// <https://github.com/opencontainers/image-spec/blob/v1.1.0-rc5/image-index.md>
    fn platform(&self) -> &Platform;

    /// Returns the container id for the running container
    fn container_id(&self) -> &str;

    /// Returns the pod id for the running container (if available)
    /// In Kubernetes environments, containers run within pods, and the pod ID is usually
    /// stored in the OCI spec annotations under "io.kubernetes.cri.sandbox-id"
    fn pod_id(&self) -> Option<&str> {
        None
    }
}

/// The source for a WASI module / components.
#[derive(Debug)]
pub enum Source<'a> {
    /// The WASI module is a file in the file system.
    File(PathBuf),
    /// The WASI module / component is provided as a layer in the OCI spec.
    /// For a WASI preview 1 module this is usually a single element array.
    /// For a WASI preview 2 component this is an array of one or more
    /// elements, where each element is a component.
    /// Runtimes can additionally provide a list of layer types they support,
    /// and they will be included in this array, e.g., a `toml` file with the
    /// runtime configuration.
    Oci(&'a [WasmLayer]),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WasmLayer {
    pub config: Descriptor,
    #[serde(with = "serde_bytes")]
    pub layer: Vec<u8>,
}

impl<'a> Source<'a> {
    /// Returns the bytes of the WASI module / component.
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
pub struct Entrypoint<'a> {
    /// The name of the exported function to call. Defaults to "_start".
    pub func: String,
    /// The name of the WASI module / component without the extension.
    pub name: Option<String>,
    /// The first argument in the OCI spec for entrypoint.
    pub arg0: Option<&'a Path>,
    /// The source of the WASI module / component, either a file or an OCI layer.
    pub source: Source<'a>,
}

pub(crate) struct WasiContext<'a> {
    pub spec: &'a Spec,
    pub wasm_layers: &'a [WasmLayer],
    pub platform: &'a Platform,
    pub id: String,
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
            .map(|a| a.as_slice())
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

    fn container_id(&self) -> &str {
        &self.id
    }

    fn pod_id(&self) -> Option<&str> {
        pod_id(self.spec)
    }
}

pub(crate) fn pod_id(spec: &Spec) -> Option<&str> {
    spec.annotations()
        .as_ref()
        .and_then(|a| a.get("io.kubernetes.cri.sandbox-id"))
        .map(|s| s.as_str())
}

/// The type of a wasm binary.
pub enum WasmBinaryType {
    /// A wasm module.
    Module,
    /// A wasm component.
    Component,
}

impl WasmBinaryType {
    /// Returns the type of the wasm binary.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if Parser::is_component(bytes) {
            Some(Self::Component)
        } else if Parser::is_core_wasm(bytes) {
            Some(Self::Module)
        } else {
            None
        }
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
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
            id: "test".to_string(),
        };

        let envs = ctx.envs();
        assert_eq!(envs.len(), 2);

        Ok(())
    }

    #[test]
    fn test_get_pod_id() -> Result<()> {
        use std::collections::HashMap;

        let mut annotations = HashMap::new();
        annotations.insert(
            "io.kubernetes.cri.sandbox-id".to_string(),
            "test-pod-id".to_string(),
        );

        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(ProcessBuilder::default().cwd("/").build()?)
            .annotations(annotations)
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            wasm_layers: &[],
            platform: &Platform::default(),
            id: "test-container".to_string(),
        };

        assert_eq!(ctx.pod_id(), Some("test-pod-id"));

        Ok(())
    }

    #[test]
    fn test_get_pod_id_no_annotation() -> Result<()> {
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(ProcessBuilder::default().cwd("/").build()?)
            .build()?;

        let ctx = WasiContext {
            spec: &spec,
            wasm_layers: &[],
            platform: &Platform::default(),
            id: "test-container".to_string(),
        };

        assert_eq!(ctx.pod_id(), None);

        Ok(())
    }
}
