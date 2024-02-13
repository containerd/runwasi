//! Testing utilities used across different modules

use std::collections::HashMap;
use std::fs::{self, create_dir, read_to_string, write, File};
use std::marker::PhantomData;
use std::ops::Add;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Result};
pub use containerd_shim_wasm_test_modules as modules;
use oci_spec::image::{self as spec, Arch};
use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
use oci_tar_builder::{Builder, WASM_LAYER_MEDIA_TYPE};

use crate::sandbox::{Instance, InstanceConfig};
use crate::sys::signals::SIGKILL;

const TEST_NAMESPACE: &str = "runwasi-test";

pub struct WasiTestBuilder<WasiInstance: Instance>
where
    WasiInstance::Engine: Default + Send + Sync + Clone,
{
    container_name: String,
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
            container_name: "test".to_string(),
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

    pub fn as_oci_image(
        mut self,
        image_name: Option<String>,
        container_name: Option<String>,
    ) -> Result<(Self, oci_helpers::OCICleanup)> {
        let mut builder = Builder::default();

        let dir = self.tempdir.path();
        let wasm_path = dir.join("rootfs").join("hello.wasm");
        builder.add_layer_with_media_type(&wasm_path, WASM_LAYER_MEDIA_TYPE.to_string());

        let config = spec::ConfigBuilder::default()
            .entrypoint(vec!["_start".to_string()])
            .build()
            .unwrap();

        let img = spec::ImageConfigurationBuilder::default()
            .config(config)
            .os("wasip1")
            .architecture(Arch::Wasm)
            .rootfs(
                spec::RootFsBuilder::default()
                    .diff_ids(vec![])
                    .build()
                    .unwrap(),
            )
            .build()?;

        let image_name = image_name.unwrap_or("localhost/hello:latest".to_string());
        builder.add_config(img, image_name.clone());

        let img = dir.join("img.tar");
        let f = File::create(img.clone())?;
        builder.build(f)?;

        let success = Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("image")
            .arg("import")
            .arg("--all-platforms")
            .arg(img)
            .spawn()?
            .wait()?
            .success();

        if !success {
            // if the container still exists try cleaning it up
            bail!(" failed to import image");
        }

        fs::remove_file(&wasm_path)?;

        let container_name = container_name.unwrap_or("test".to_string());
        let success = Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("c")
            .arg("create")
            .arg(&image_name)
            .arg(&container_name)
            .spawn()?
            .wait()?
            .success();

        if !success {
            bail!(" failed to create container for image");
        }

        self.container_name = container_name.clone();
        Ok((
            self,
            oci_helpers::OCICleanup {
                image_name,
                container_name,
            },
        ))
    }

    pub fn build(self) -> Result<WasiTest<WasiInstance>> {
        let tempdir = self.tempdir;
        let dir = tempdir.path();

        log::info!("building wasi test");

        let mut cfg = InstanceConfig::new(
            WasiInstance::Engine::default(),
            TEST_NAMESPACE,
            "/run/containerd/containerd.sock",
        );
        cfg.set_bundle(dir)
            .set_stdout(dir.join("stdout"))
            .set_stderr(dir.join("stderr"))
            .set_stdin(dir.join("stdin"));

        let instance = WasiInstance::new(self.container_name, Some(&cfg))?;
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
        let (status, _) = match self.instance.wait_timeout(timeout) {
            Some(res) => res,
            None => {
                self.instance.kill(SIGKILL as u32)?;
                bail!("timeout while waiting for module to finish");
            }
        };

        let stdout = read_to_string(dir.join("stdout"))?;
        let stderr = read_to_string(dir.join("stderr"))?;

        self.instance.delete()?;

        log::info!("wasi test status is {status}");

        Ok((status, stdout, stderr))
    }
}

pub mod oci_helpers {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use anyhow::{bail, Result};

    use super::TEST_NAMESPACE;

    pub struct OCICleanup {
        pub image_name: String,
        pub container_name: String,
    }

    impl Drop for OCICleanup {
        fn drop(&mut self) {
            log::debug!("dropping OCIGuard");
            clean_container(self.container_name.clone()).unwrap();
            clean_image(self.image_name.clone()).unwrap();
        }
    }

    pub fn clean_container(container_name: String) -> Result<()> {
        log::debug!("deleting container '{}'", container_name);
        let success = Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("c")
            .arg("rm")
            .arg(container_name)
            .spawn()?
            .wait()?
            .success();

        if !success {
            bail!("failed to clean container")
        }

        Ok(())
    }

    pub fn clean_image(image_name: String) -> Result<()> {
        log::debug!("deleting image '{}'", image_name);
        let success = Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("i")
            .arg("rm")
            .arg(image_name)
            .spawn()?
            .wait()?
            .success();

        if !success {
            bail!("failed to clean image");
        }

        // the content isn't removed immediately, so we need to wait for it to be removed
        // otherwise the next test will not behave as expected
        let start = Instant::now();
        let timeout = Duration::from_secs(300);
        loop {
            let output = Command::new("ctr")
                .arg("-n")
                .arg(TEST_NAMESPACE)
                .arg("content")
                .arg("ls")
                .arg("-q")
                .output()?;

            if output.stdout.is_empty() {
                break;
            }

            if start.elapsed() > timeout {
                bail!("timed out waiting for content to be removed");
            }

            log::trace!("waiting for content to be removed");
        }

        Ok(())
    }

    pub fn get_image_label() -> Result<(String, String)> {
        let mut grep = Command::new("grep")
            .arg("-ohE")
            .arg("runwasi.io/precompiled/.*")
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()?;

        Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("i")
            .arg("ls")
            .stdout(grep.stdin.take().unwrap())
            .spawn()?;

        let output = grep.wait_with_output()?;

        let stdout = String::from_utf8(output.stdout)?;

        log::info!("stdout: {}", stdout);

        let label: Vec<&str> = stdout.split('=').collect();

        Ok((
            label.first().unwrap().trim().to_string(),
            label.last().unwrap().trim().to_string(),
        ))
    }

    pub fn remove_content(digest: String) -> Result<()> {
        log::debug!("cleaning content '{}'", digest);
        let success = Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("content")
            .arg("rm")
            .arg(digest)
            .spawn()?
            .wait()?
            .success();

        if !success {
            bail!("failed to remove content");
        }

        Ok(())
    }
}
