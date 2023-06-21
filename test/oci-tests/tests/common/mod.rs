use std::fs::{create_dir, read_to_string, File, OpenOptions};
use std::io::prelude::*;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use libc::SIGKILL;
use oci_spec::runtime::Spec;
use serde::{Deserialize, Serialize};
use tempfile::tempdir;

use containerd_shim_wasm::sandbox::instance::Wait;
use containerd_shim_wasm::sandbox::{EngineGetter, Error, Instance, InstanceConfig};

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

pub static WASM_FILENAME: &str = "./file.wasm";

pub(crate) fn get_external_wasm_module(name: String) -> Result<Vec<u8>, Error> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let target = Path::new(manifest_dir)
        .join("../../target/wasm32-wasi/debug")
        .join(name.clone());
    std::fs::read(target).map_err(|e| {
            Error::Others(format!(
                "failed to read requested Wasm module ({}): {}. Perhaps you need to run 'make test/wasm-modules' first.",
                name, e
            ))
        })
}

pub(crate) fn run_test_with_spec<I, E>(spec: &Spec, bytes: &[u8]) -> Result<(String, u32), Error>
where
    I: Instance<E = E> + EngineGetter<E = E>,
    E: Sync + Send + Clone,
{
    let dir = tempdir().unwrap();
    create_dir(dir.path().join("rootfs"))?;
    let rootdir = dir.path().join("runwasi");
    create_dir(&rootdir)?;
    let opts = Options {
        root: Some(rootdir),
    };
    let opts_file = OpenOptions::new()
        .read(true)
        .create(true)
        .truncate(true)
        .write(true)
        .open(dir.path().join("options.json"))?;
    write!(&opts_file, "{}", serde_json::to_string(&opts)?)?;

    let wasm_path = dir.path().join("rootfs").join(WASM_FILENAME);
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(wasm_path)?;
    f.write_all(bytes)?;

    let stdout = File::create(dir.path().join("stdout"))?;
    drop(stdout);

    spec.save(dir.path().join("config.json"))?;

    let mut cfg = InstanceConfig::new(
        I::new_engine()?,
        "test_namespace".into(),
        "/containerd/address".into(),
    );
    let cfg = cfg
        .set_bundle(dir.path().to_str().unwrap().to_string())
        .set_stdout(dir.path().join("stdout").to_str().unwrap().to_string());

    let wasi = I::new("test".to_string(), Some(cfg));

    wasi.start()?;

    let (tx, rx) = channel();
    let waiter = Wait::new(tx);
    wasi.wait(&waiter).unwrap();

    let res = match rx.recv_timeout(Duration::from_secs(600)) {
        Ok(res) => res,
        Err(e) => {
            wasi.kill(SIGKILL as u32).unwrap();
            return Err(Error::Others(format!(
                "error waiting for module to finish: {0}",
                e
            )));
        }
    };
    wasi.delete()?;
    let output = read_to_string(dir.path().join("stdout"))?;
    Ok((output, res.0))
}
