use std::collections::BTreeMap;
use std::hash::Hash;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as symlink;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use containerd_shim_wasm::sandbox::context::WasmLayer;
use containerd_shim_wasm::shim::Compiler;
use containerd_shim_wasm::shim::cache::{CacheConfig, CachedCompiler, ModuleCache};
use oci_spec::image::{Descriptor, Digest, MediaType};
use tempfile::TempDir;

#[derive(Clone, Default)]
struct RecordingCompiler(Arc<Mutex<Vec<Vec<u8>>>>);

impl Compiler for RecordingCompiler {
    fn cache_key(&self) -> impl Hash {
        "regular-file-v1"
    }

    async fn compile(&self, layers: &[WasmLayer]) -> Result<Vec<Option<Vec<u8>>>> {
        self.0
            .lock()
            .unwrap()
            .extend(layers.iter().map(|layer| layer.layer.clone()));
        Ok(layers
            .iter()
            .map(|layer| Some(compiled(&layer.layer)))
            .collect())
    }
}

fn layer(bytes: &[u8]) -> WasmLayer {
    WasmLayer {
        config: Descriptor::new(
            MediaType::Other("application/wasm".to_string()),
            bytes.len() as u64,
            Digest::try_from(format!("sha256:{:064x}", 0)).unwrap(),
        ),
        precompiled: false,
        layer: bytes.to_vec(),
    }
}

fn compiled(bytes: &[u8]) -> Vec<u8> {
    [b"compiled:".as_slice(), bytes].concat()
}

fn open(path: &Path) -> ModuleCache {
    ModuleCache::open(CacheConfig {
        dir: path.to_path_buf(),
        ttl: Duration::from_secs(3600),
        max_bytes: 1024 * 1024,
    })
    .unwrap()
}

fn files(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_file())
        .map(|entry| {
            let path = entry.path();
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn an_open_cache_rejects_a_layer_symlink_to_matching_bytes() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let inner = RecordingCompiler::default();
    let cached = CachedCompiler::new(inner.clone(), Some(open(dir.path())));
    cached.compile(&[layer(b"aaa")]).await.unwrap();

    let expected = compiled(b"aaa");
    let stored = files(dir.path())
        .into_iter()
        .find_map(|(path, bytes)| (bytes == expected).then_some(path))
        .expect("the compiled layer must be stored as its bytes");
    let target = outside.path().join("matching-layer");
    std::fs::write(&target, &expected).unwrap();
    std::fs::remove_file(&stored).unwrap();
    symlink(&target, &stored).unwrap();
    assert_eq!(std::fs::read(&stored).unwrap(), expected);

    assert_eq!(
        cached.compile(&[layer(b"aaa")]).await.unwrap(),
        vec![Some(compiled(b"aaa"))]
    );
    assert_eq!(*inner.0.lock().unwrap(), vec![b"aaa".to_vec(), b"aaa".to_vec()]);
    let observed = cached.stats().unwrap();
    assert_eq!(observed.hits, 0);
    assert_eq!(observed.misses, 2);
    assert_eq!(observed.compilations, 2);
    assert!(observed.rejected >= 1);
}

#[tokio::test(flavor = "current_thread")]
async fn symlinked_bookkeeping_is_absent_when_opening_or_using_a_cache() {
    for reopen in [false, true] {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let inner = RecordingCompiler::default();
        let mut cached = CachedCompiler::new(inner.clone(), Some(open(dir.path())));
        let before = files(dir.path());
        cached.compile(&[layer(b"aaa")]).await.unwrap();

        // Only replace persisted records changed by the store, leaving the raw
        // layer and unchanged coordination files alone. No format is assumed.
        let records: Vec<_> = files(dir.path())
            .into_iter()
            .filter(|(path, bytes)| {
                !bytes.is_empty()
                    && *bytes != compiled(b"aaa")
                    && before.get(path) != Some(bytes)
            })
            .collect();
        if records.is_empty() {
            // There is no separate entry bookkeeping to replace in this layout.
            continue;
        }
        for (index, (path, bytes)) in records.into_iter().enumerate() {
            let target = outside.path().join(format!("record-{index}"));
            std::fs::write(&target, &bytes).unwrap();
            std::fs::remove_file(&path).unwrap();
            symlink(&target, &path).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }

        if reopen {
            drop(cached);
            cached = CachedCompiler::new(inner.clone(), Some(open(dir.path())));
        }
        let before_lookup = cached.stats().unwrap();
        assert_eq!(before_lookup.entries, 0);
        assert_eq!(before_lookup.bytes, 0);
        assert_eq!(
            cached.compile(&[layer(b"aaa")]).await.unwrap(),
            vec![Some(compiled(b"aaa"))]
        );
        assert_eq!(*inner.0.lock().unwrap(), vec![b"aaa".to_vec(), b"aaa".to_vec()]);
        let observed = cached.stats().unwrap();
        assert_eq!(observed.hits, 0);
        assert_eq!(observed.misses, if reopen { 1 } else { 2 });
    }
}
