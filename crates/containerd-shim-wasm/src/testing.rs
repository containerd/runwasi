//! Testing utilities used across different modules

use std::collections::HashMap;
use std::fs::{self, File, create_dir, read, read_to_string, write};
use std::marker::PhantomData;
use std::ops::Add;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as symlink;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
pub use containerd_shim_wasm_test_modules as modules;
use containerd_shimkit::AmbientRuntime as _;
use containerd_shimkit::sandbox::{Instance as _, InstanceConfig};
use libc::{SIGINT, SIGTERM};
use oci_spec::runtime::{
    LinuxBuilder, LinuxNamespace, LinuxNamespaceType, ProcessBuilder, RootBuilder, SpecBuilder,
    get_default_namespaces,
};

use crate::container::{Engine, Instance};

pub const TEST_NAMESPACE: &str = "runwasi-test";
pub const SIGKILL: u32 = 9;

pub struct WasiTestBuilder<WasiEngine: Engine + Default> {
    container_name: String,
    start_fn: String,
    namespaces: Vec<LinuxNamespace>,
    tempdir: tempfile::TempDir,
    _phantom: PhantomData<WasiEngine>,
}

pub struct WasiTest<WasiEngine: Engine + Default> {
    instance: Instance<WasiEngine>,
    tempdir: tempfile::TempDir,
}

impl<WasiEngine: Engine + Default> WasiTestBuilder<WasiEngine> {
    pub fn new() -> Result<Self> {
        containerd_shimkit::zygote::Zygote::init();

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
            container_name: "test".to_string(),
            start_fn: "".to_string(),
            namespaces: get_default_namespaces(),
            _phantom: Default::default(),
        }
        .with_wasm([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])?
        .with_stdin("")?;

        Ok(builder)
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.container_name = name.into();
        self
    }

    pub fn with_host_network(mut self) -> Self {
        // Removing the `network` namespace results in the binding to the host's socket.
        // This allows for direct communication with the host's networking interface.
        self.namespaces
            // typos:disable-next-line - false positive "typ"
            .retain(|ns| ns.typ() != LinuxNamespaceType::Network);
        self
    }

    pub fn with_start_fn(mut self, start_fn: impl AsRef<str>) -> Self {
        start_fn.as_ref().clone_into(&mut self.start_fn);
        self
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

    pub fn with_stdout(self, stdout: impl AsRef<Path>) -> Result<Self> {
        let stdout = fs::canonicalize(stdout.as_ref())?;

        let dir = self.tempdir.path();

        log::info!("setting wasi test stdout to {:?}", stdout);

        std::fs::remove_file(dir.join("stdout"))?;
        symlink(stdout, dir.join("stdout"))?;

        Ok(self)
    }

    pub fn with_stderr(self, stderr: impl AsRef<Path>) -> Result<Self> {
        let stderr = fs::canonicalize(stderr.as_ref())?;

        let dir = self.tempdir.path();

        log::info!("setting wasi test stderr to {:?}", stderr);

        std::fs::remove_file(dir.join("stderr"))?;
        symlink(stderr, dir.join("stderr"))?;

        Ok(self)
    }

    pub fn as_oci_image(
        mut self,
        image_name: Option<String>,
        container_name: Option<String>,
    ) -> Result<(Self, oci_helpers::OCICleanup)> {
        let image_name = image_name.unwrap_or("localhost/hello:latest".to_string());

        if !oci_helpers::image_exists(&image_name) {
            let wasm_path = self.tempdir.path().join("rootfs").join("hello.wasm");
            let bytes = read(&wasm_path)?;
            let wasm_content = oci_helpers::ImageContent {
                bytes,
                media_type: oci_tar_builder::WASM_LAYER_MEDIA_TYPE.to_string(),
            };
            oci_helpers::import_image(&image_name, &[&wasm_content])?;

            // remove the file from the rootfs so it doesn't get treated like a regular container
            fs::remove_file(&wasm_path)?;
        }

        let container_name = container_name.unwrap_or("test".to_string());
        oci_helpers::create_container(&container_name, &image_name)?;

        self.container_name.clone_from(&container_name);
        Ok((
            self,
            oci_helpers::OCICleanup {
                image_name,
                container_name,
            },
        ))
    }

    pub fn build(self) -> Result<WasiTest<WasiEngine>> {
        let tempdir = self.tempdir;
        let dir = tempdir.path();

        log::info!("setting wasi test start_fn to {}", self.start_fn);

        let entrypoint = match self.start_fn.as_str() {
            "" => "/hello.wasm".to_string(),
            s => "/hello.wasm#".to_string().add(s),
        };

        let spec = SpecBuilder::default()
            .root(RootBuilder::default().path("rootfs").build()?)
            .linux(
                LinuxBuilder::default()
                    .namespaces(self.namespaces)
                    .build()?,
            )
            .process(
                ProcessBuilder::default()
                    .cwd("/")
                    .args([entrypoint])
                    .build()?,
            )
            .build()?;

        spec.save(dir.join("config.json"))?;

        log::info!("building wasi test: {}", dir.display());

        let cfg = InstanceConfig {
            namespace: TEST_NAMESPACE.to_string(),
            containerd_address: "/run/containerd/containerd.sock".to_string(),
            bundle: dir.to_path_buf(),
            stdout: dir.join("stdout"),
            stderr: dir.join("stderr"),
            stdin: dir.join("stdin"),
            ..Default::default()
        };

        let instance = Instance::<WasiEngine>::new(self.container_name, &cfg).block_on()?;
        Ok(WasiTest { instance, tempdir })
    }
}

impl<WasiEngine: Engine + Default> WasiTest<WasiEngine> {
    pub fn builder() -> Result<WasiTestBuilder<WasiEngine>> {
        WasiTestBuilder::new()
    }

    pub fn instance(&self) -> &Instance<WasiEngine> {
        &self.instance
    }

    pub fn start(&self) -> Result<&Self> {
        log::info!("starting wasi test");
        let pid = self.instance.start().block_on()?;
        log::info!("wasi test pid {pid}");

        Ok(self)
    }

    pub fn delete(&self) -> Result<&Self> {
        log::info!("deleting wasi test");
        self.instance.delete().block_on()?;
        Ok(self)
    }

    pub fn ctrl_c(&self) -> Result<&Self> {
        log::info!("sending SIGINT");
        self.instance.kill(SIGINT as u32).block_on()?;
        Ok(self)
    }

    pub fn terminate(&self) -> Result<&Self> {
        log::info!("sending SIGTERM");
        self.instance.kill(SIGTERM as u32).block_on()?;
        Ok(self)
    }

    pub fn kill(&self) -> Result<&Self> {
        log::info!("sending SIGKILL");
        self.instance.kill(SIGKILL as u32).block_on()?;
        Ok(self)
    }

    pub fn wait(&self, t: Duration) -> Result<(u32, String, String)> {
        log::info!("waiting wasi test");
        let (status, _) = match self.instance.wait().with_timeout(t).block_on() {
            Some(res) => res,
            None => {
                self.instance.kill(SIGKILL).block_on()?;
                bail!("timeout while waiting for module to finish");
            }
        };

        let stdout = self.read_stdout()?.unwrap_or_default();
        let stderr = self.read_stderr()?.unwrap_or_default();

        self.instance.delete().block_on()?;

        log::info!("wasi test status is {status}");

        Ok((status, stdout, stderr))
    }

    pub fn root(&self) -> &Path {
        self.tempdir.path()
    }

    pub fn read_stdout(&self) -> Result<Option<String>> {
        let path = self.tempdir.path().join("stdout");
        if path.is_symlink() {
            return Ok(None);
        }
        Ok(Some(read_to_string(path)?))
    }

    pub fn read_stderr(&self) -> Result<Option<String>> {
        let path = self.tempdir.path().join("stderr");
        if path.is_symlink() {
            return Ok(None);
        }
        Ok(Some(read_to_string(path)?))
    }
}

pub mod oci_helpers {
    use std::fs::{File, write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use anyhow::{Result, bail};
    use oci_spec::image::{self as spec, Arch};
    use oci_tar_builder::Builder;

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

    pub fn create_container(container_name: &str, image_name: &str) -> Result<(), anyhow::Error> {
        let success = Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("c")
            .arg("create")
            .arg(image_name)
            .arg(container_name)
            .spawn()?
            .wait()?
            .success();
        if !success {
            bail!(" failed to create container for image");
        }
        Ok(())
    }

    pub struct ImageContent {
        pub bytes: Vec<u8>,
        pub media_type: String,
    }

    pub fn import_image(
        image_name: &str,
        wasm_content: &[&ImageContent],
    ) -> Result<(), anyhow::Error> {
        let tempdir = tempfile::tempdir()?;
        let dir = tempdir.path();

        let mut builder = Builder::default();

        for (i, content) in wasm_content.iter().enumerate() {
            let path = tempdir.path().join(format!("{}.wasm", i));
            write(path.clone(), content.bytes.clone())?;
            builder.add_layer_with_media_type(&path, content.media_type.clone());
        }

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
        builder.add_config(img, image_name.to_string(), spec::MediaType::ImageConfig);
        let img_path = dir.join("img.tar");
        let f = File::create(img_path.clone())?;
        builder.build(f)?;

        let success = Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("image")
            .arg("import")
            .arg("--all-platforms")
            .arg(img_path)
            .spawn()?
            .wait()?
            .success();
        if !success {
            // if the container still exists try cleaning it up
            bail!(" failed to import image");
        };
        Ok(())
    }

    pub fn clean_image(image_name: String) -> Result<()> {
        let image_sha = match get_image_sha(&image_name) {
            Ok(sha) => sha,
            Err(_) => return Ok(()), // doesn't exist
        };

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

        // the content is not removed immediately, so we need to wait for it to be removed
        // otherwise some tests will not behave as expected
        wait_for_content_removal(&image_sha)?;

        Ok(())
    }

    pub fn wait_for_content_removal(content_sha: &str) -> Result<(), anyhow::Error> {
        let start = Instant::now();
        let timeout = Duration::from_secs(60);
        log::info!("waiting for content to be removed: {}", &content_sha);
        loop {
            let output = Command::new("ctr")
                .arg("-n")
                .arg(TEST_NAMESPACE)
                .arg("content")
                .arg("get")
                .arg(content_sha)
                .output()?;

            if output.stdout.is_empty() {
                break;
            }

            if start.elapsed() > timeout {
                log::warn!("didn't clean content fully");
                break;
            }
        }
        Ok(())
    }

    fn get_image_sha(image_name: &str) -> Result<String> {
        log::info!("getting image sha for '{}'", image_name);
        let mut grep = Command::new("grep")
            .arg(image_name)
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
        log::warn!("stdout: {}", stdout);

        let parts: Vec<&str> = stdout.trim().split(' ').collect();
        if parts.len() < 3 {
            bail!("failed to get image sha");
        }
        let sha = parts[2];
        log::warn!("sha: {}", sha);
        Ok(sha.to_string())
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
        log::debug!("stdout: {}", stdout);
        let label: Vec<&str> = stdout.split('=').collect();

        Ok((
            label.first().unwrap().trim().to_string(),
            label.last().unwrap().trim().to_string(),
        ))
    }

    pub fn image_exists(image_name: &str) -> bool {
        let output = Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("i")
            .arg("ls")
            .arg("--quiet")
            .output()
            .expect("failed to get output of image list");

        let stdout = String::from_utf8(output.stdout).expect("failed to parse stdout");
        stdout.contains(image_name)
    }

    pub fn get_content_label() -> Result<(String, String)> {
        let mut grep = Command::new("grep")
            .arg("-ohE")
            .arg("runwasi.io/precompiled/[[:alpha:]]*/[0-9]+=.*")
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()?;

        Command::new("ctr")
            .arg("-n")
            .arg(TEST_NAMESPACE)
            .arg("content")
            .arg("ls")
            .stdout(grep.stdin.take().unwrap())
            .spawn()?;

        let output = grep.wait_with_output()?;

        let stdout = String::from_utf8(output.stdout)?;

        log::debug!("stdout: {}", stdout);

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
