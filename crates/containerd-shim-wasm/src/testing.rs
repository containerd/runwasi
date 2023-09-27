//! Testing utilities used across different modules

use std::collections::HashMap;
use std::fs::{create_dir, read_to_string, write, File};
use std::marker::PhantomData;
use std::ops::Add;
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::{bail, Result};
pub use containerd_shim_wasm_test_modules as modules;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};

use crate::sandbox::instance::Wait;
use crate::sandbox::{Instance, InstanceConfig};
use crate::sys::signals::SIGKILL;

pub struct WasiTestBuilder<WasiInstance: Instance>
where
    WasiInstance::Engine: Default + Send + Sync + Clone,
{
    tempdir: tempfile::TempDir,
    _phantom: PhantomData<WasiInstance>,
}

pub struct WasiTest<WasiInstance: Instance>
where
    WasiInstance::Engine: Default + Send + Sync + Clone,
{
    instance: WasiInstance,
    tempdir: tempfile::TempDir,
}

impl<WasiInstance: Instance> WasiTestBuilder<WasiInstance>
where
    WasiInstance::Engine: Default + Send + Sync + Clone,
{
    pub fn new() -> Result<Self> {
        // start logging
        // to enable logging run `export RUST_LOG=trace` and append cargo command with
        // --show-output before running test
        let _ = env_logger::try_init();

        log::info!("creating new wasi test");

        let tempdir = tempfile::tempdir()?;
        let dir = tempdir.path();

        create_dir(dir.join("rootfs"))?;
        let rootdir = dir.join("runwasi");
        create_dir(&rootdir)?;
        let opts = HashMap::from([("root", rootdir)]);
        let opts_file = File::create(dir.join("options.json"))?;
        serde_json::to_writer(opts_file, &opts)?;

        write(dir.join("stdout"), "")?;
        write(dir.join("stderr"), "")?;

        let builder = Self {
            tempdir,
            _phantom: Default::default(),
        }
        .with_wasm([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])?
        .with_start_fn("")?
        .with_stdin("")?;

        Ok(builder)
    }

    pub fn with_start_fn(self, start_fn: impl AsRef<str>) -> Result<Self> {
        let dir = self.tempdir.path();
        let start_fn = start_fn.as_ref();

        log::info!("setting wasi test start_fn to {start_fn:?}");

        let entrypoint = match start_fn {
            "" => "/hello.wasm".to_string(),
            s => "/hello.wasm#".to_string().add(s),
        };
        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args([entrypoint])
                    .build()?,
            )
            .build()?;

        spec.save(dir.join("config.json"))?;

        Ok(self)
    }

    pub fn with_wasm(self, wasmbytes: impl AsRef<[u8]>) -> Result<Self> {
        let dir = self.tempdir.path();

        log::info!(
            "setting wasi test wasm file [u8; {}]",
            wasmbytes.as_ref().len()
        );

        let wasm_path = dir.join("rootfs").join("hello.wasm");
        write(wasm_path, wasmbytes)?;

        Ok(self)
    }

    pub fn with_stdin(self, stdin: impl AsRef<[u8]>) -> Result<Self> {
        let dir = self.tempdir.path();

        log::info!("setting wasi test stdin to [u8; {}]", stdin.as_ref().len());

        write(dir.join("stdin"), stdin)?;

        Ok(self)
    }

    pub fn build(self) -> Result<WasiTest<WasiInstance>> {
        let tempdir = self.tempdir;
        let dir = tempdir.path();

        log::info!("building wasi test");

        let mut cfg = InstanceConfig::new(
            WasiInstance::Engine::default(),
            "test_namespace".into(),
            "/containerd/address".into(),
        );
        cfg.set_bundle(dir.to_string_lossy().to_string())
            .set_stdout(dir.join("stdout").to_string_lossy().to_string())
            .set_stderr(dir.join("stderr").to_string_lossy().to_string())
            .set_stdin(dir.join("stdin").to_string_lossy().to_string());

        let instance = WasiInstance::new("test".to_string(), Some(&cfg))?;
        Ok(WasiTest { instance, tempdir })
    }
}

impl<WasiInstance: Instance> WasiTest<WasiInstance>
where
    WasiInstance::Engine: Default + Send + Sync + Clone,
{
    pub fn builder() -> Result<WasiTestBuilder<WasiInstance>> {
        WasiTestBuilder::new()
    }

    pub fn instance(&self) -> &WasiInstance {
        &self.instance
    }

    pub fn start(&self) -> Result<&Self> {
        log::info!("starting wasi test");
        self.instance.start()?;
        Ok(self)
    }

    pub fn delete(&self) -> Result<&Self> {
        log::info!("deleting wasi test");
        self.instance.delete()?;
        Ok(self)
    }

    pub fn wait(&self, timeout: Duration) -> Result<(u32, String, String)> {
        let dir = self.tempdir.path();

        log::info!("waiting wasi test");

        let (tx, rx) = channel();
        let waiter = Wait::new(tx);
        self.instance.wait(&waiter).unwrap();

        let (status, _) = match rx.recv_timeout(timeout) {
            Ok(res) => res,
            Err(e) => {
                self.instance.kill(SIGKILL as u32)?;
                bail!("error waiting for module to finish: {e}");
            }
        };

        let stdout = read_to_string(dir.join("stdout"))?;
        let stderr = read_to_string(dir.join("stderr"))?;

        self.instance.delete()?;

        log::info!("wasi test status is {status}");

        Ok((status, stdout, stderr))
    }
}
