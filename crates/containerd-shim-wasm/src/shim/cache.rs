//! An on-disk cache of precompiled layers.
//!
//! Shims that precompile their layers pay the compilation cost again whenever
//! the containerd content store does not hold a precompiled copy, for example
//! the first time an image is run on a node. [`CachedCompiler`] wraps the shim
//! [`Compiler`] with a [`ModuleCache`], a directory of precompiled layers keyed
//! by the layer contents and the cache key of the compiler, so that the work is
//! shared across images and across shim processes.
//!
//! Every shim process on a node runs against the same directory. The manifest
//! is rewritten under an exclusive lock and published by rename, so a reader
//! either sees the manifest as it was or as it became, and a writer merges into
//! whatever the directory holds rather than over it. A second lock is held for
//! the length of a compilation, so a shim that wants a layer another one is
//! already compiling waits for it instead of compiling it again.
//!
//! The cache is an optimization and never a reason to fail a container: a
//! directory that cannot be read or written turns every lookup into a miss and
//! drops every store, leaving the wrapped compiler to answer on its own.
//!
//! The cache is opt-in and configured through the environment:
//!
//! - `RUNWASI_MODULE_CACHE_DIR`: the directory holding the cache. The cache is
//!   disabled unless this is set to a non-empty value.
//! - `RUNWASI_MODULE_CACHE_TTL_SECS`: how long an entry stays usable after it
//!   was stored. Defaults to a day.
//! - `RUNWASI_MODULE_CACHE_MAX_BYTES`: the upper bound on the size of the
//!   stored layers. Defaults to 1 GiB.

use std::collections::{HashMap, hash_map};
use std::fs::{File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher as _};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use oci_spec::runtime::Spec;
use serde::{Deserialize, Serialize};
use sha256::digest;

use crate::sandbox::context::WasmLayer;
use crate::shim::{Compiler, Shim};

static CACHE_DIR_ENV: &str = "RUNWASI_MODULE_CACHE_DIR";
static CACHE_TTL_ENV: &str = "RUNWASI_MODULE_CACHE_TTL_SECS";
static CACHE_MAX_BYTES_ENV: &str = "RUNWASI_MODULE_CACHE_MAX_BYTES";

static MANIFEST_FILE: &str = "manifest.json";
static MANIFEST_LOCK_FILE: &str = "manifest.lock";
static BUILD_LOCK_FILE: &str = "build.lock";
static PENDING_SUFFIX: &str = ".pending";

static DEFAULT_TTL_SECS: u64 = 24 * 60 * 60;
static DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// Configuration of the on-disk cache of precompiled layers.
#[derive(Debug)]
pub struct CacheConfig {
    /// The directory holding the cached layers and the manifest.
    pub dir: PathBuf,
    /// How long an entry stays usable after it was stored.
    pub ttl: Duration,
    /// The upper bound on the total size of the stored layers.
    pub max_bytes: u64,
}

impl CacheConfig {
    /// Reads the configuration from the environment, returning `None` when the
    /// cache is not enabled.
    pub fn from_env() -> Option<Self> {
        let dir = std::env::var_os(CACHE_DIR_ENV).filter(|dir| !dir.is_empty())?;
        Some(Self {
            dir: dir.into(),
            ttl: Duration::from_secs(parse_env(CACHE_TTL_ENV, DEFAULT_TTL_SECS)),
            max_bytes: parse_env(CACHE_MAX_BYTES_ENV, DEFAULT_MAX_BYTES),
        })
    }
}

/// What the cache directory holds, and what a [`ModuleCache`] has done with it
/// since it was opened.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// The number of layers currently stored.
    pub entries: u64,
    /// The total size of the layers currently stored.
    pub bytes: u64,
    /// The number of layers served from the cache.
    pub hits: u64,
    /// The number of lookups the cache could not serve.
    pub misses: u64,
    /// The number of entries dropped to stay within `max_bytes`.
    pub evictions: u64,
    /// The number of entries dropped for having outlived the TTL.
    pub expirations: u64,
    /// The number of entries dropped for not reading back as they were stored.
    pub rejected: u64,
    /// The number of files removed for belonging to no entry.
    pub swept: u64,
    /// The number of layers handed to the wrapped compiler.
    pub compilations: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    size: u64,
    digest: String,
    stored: u64,
    /// Where this entry sits in the directory's order of use. A counter rather
    /// than a clock reading, so two uses a moment apart never tie and the least
    /// recently used entry is always the one eviction should take.
    used: u64,
}

type Manifest = HashMap<String, Entry>;

enum Lookup {
    Hit(Vec<u8>),
    Missing,
    Expired,
    Rejected,
}

#[derive(Default)]
struct Counters {
    hits: u64,
    misses: u64,
    evictions: u64,
    expirations: u64,
    rejected: u64,
    swept: u64,
    compilations: u64,
}

/// A directory of precompiled layers, shared with every other cache open on it.
pub struct ModuleCache {
    config: CacheConfig,
    counters: Mutex<Counters>,
}

impl ModuleCache {
    /// Opens the cache in `config.dir`, creating the directory if it does not
    /// exist and reconciling the manifest with the directory: entries that
    /// outlived the TTL or lost their layer go, what is left is brought back
    /// inside `max_bytes`, and so are the files left behind by a shim that died
    /// mid-store.
    pub fn open(config: CacheConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.dir)?;
        let cache = Self {
            config,
            counters: Mutex::new(Counters::default()),
        };

        let guard = cache.lock(MANIFEST_LOCK_FILE)?;
        let mut manifest = cache.read_manifest();
        let expired: Vec<_> = manifest
            .iter()
            .filter(|(_, entry)| cache.is_expired(entry))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &expired {
            cache.remove(&mut manifest, key);
        }
        let orphaned: Vec<_> = manifest
            .keys()
            .filter(|key| !is_regular_file(&cache.config.dir.join(key)))
            .cloned()
            .collect();
        for key in &orphaned {
            cache.remove(&mut manifest, key);
        }
        let holding = manifest.len();
        cache.evict(&mut manifest, 0);
        if !expired.is_empty() || !orphaned.is_empty() || manifest.len() != holding {
            cache.write_manifest(&manifest)?;
        }
        let swept = cache.sweep(&manifest)?;
        drop(guard);

        let mut counters = cache.counters.lock().unwrap();
        counters.expirations += expired.len() as u64;
        counters.rejected += orphaned.len() as u64;
        counters.swept += swept;
        drop(counters);

        Ok(cache)
    }

    /// Returns what the directory holds, and what this cache has done with it.
    pub fn stats(&self) -> CacheStats {
        let (entries, bytes) = self
            .read_manifest()
            .iter()
            .filter_map(|(key, entry)| self.read_entry(key, entry))
            .fold((0_u64, 0_u64), |(entries, bytes), stored| {
                (
                    entries.saturating_add(1),
                    bytes.saturating_add(stored.len() as u64),
                )
            });
        let counters = self.counters.lock().unwrap();
        CacheStats {
            entries,
            bytes,
            hits: counters.hits,
            misses: counters.misses,
            evictions: counters.evictions,
            expirations: counters.expirations,
            rejected: counters.rejected,
            swept: counters.swept,
            compilations: counters.compilations,
        }
    }

    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let outcome = self.probe(key);
        let mut counters = self.counters.lock().unwrap();
        match outcome {
            Lookup::Hit(bytes) => {
                counters.hits += 1;
                return Some(bytes);
            }
            Lookup::Expired => counters.expirations += 1,
            Lookup::Rejected => counters.rejected += 1,
            Lookup::Missing => {}
        }
        counters.misses += 1;
        None
    }

    /// Looks `key` up again once another cache has finished compiling. What it
    /// finds is counted, but not a second miss: the lookup that sent this cache
    /// here counted one already.
    fn recheck(&self, key: &str) -> Option<Vec<u8>> {
        let outcome = self.probe(key);
        let mut counters = self.counters.lock().unwrap();
        match outcome {
            Lookup::Hit(bytes) => {
                counters.hits += 1;
                Some(bytes)
            }
            Lookup::Expired => {
                counters.expirations += 1;
                None
            }
            Lookup::Rejected => {
                counters.rejected += 1;
                None
            }
            Lookup::Missing => None,
        }
    }

    fn probe(&self, key: &str) -> Lookup {
        let Some(entry) = self.read_manifest().remove(key) else {
            return Lookup::Missing;
        };

        if self.is_expired(&entry) {
            self.absorb(self.discard(key, &entry));
            return Lookup::Expired;
        }

        let Some(bytes) = self.read_entry(key, &entry) else {
            self.absorb(self.discard(key, &entry));
            return Lookup::Rejected;
        };

        // A hit has to leave the entry the most recently used one for every cache
        // on the directory, so a directory that will not take that update is not
        // one to go on reading from either.
        if self.absorb(self.touch(key, &entry)) != Some(true) {
            return Lookup::Missing;
        }
        Lookup::Hit(bytes)
    }

    fn read_entry(&self, key: &str, entry: &Entry) -> Option<Vec<u8>> {
        read_regular_file(&self.config.dir.join(key))
            .ok()
            .filter(|bytes| digest(bytes.as_slice()) == entry.digest)
    }

    fn put(&self, key: &str, bytes: &[u8]) {
        self.absorb(self.store(key, bytes));
    }

    /// Holds the compilation lock, so that a cache wanting a layer another one
    /// is compiling waits for it rather than compiling it a second time. The
    /// lock is held across the compilation, so waiting for it has to leave the
    /// runtime free: the cache holding it needs a thread to finish and release.
    async fn building(&self) -> Option<File> {
        let path = self.config.dir.join(BUILD_LOCK_FILE);
        let locked = tokio::task::spawn_blocking(move || lock_file(&path))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|locked| locked);
        self.absorb(locked)
    }

    fn store(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let size = bytes.len() as u64;
        if size > self.config.max_bytes {
            return Ok(());
        }

        let _guard = self.lock(MANIFEST_LOCK_FILE)?;
        let mut manifest = self.read_manifest();
        manifest.remove(key);
        self.evict(&mut manifest, size);

        atomic_write(&self.config.dir.join(key), bytes)?;

        let used = next_use(&manifest);
        manifest.insert(
            key.to_string(),
            Entry {
                size,
                digest: digest(bytes),
                stored: now(),
                used,
            },
        );
        self.write_manifest(&manifest)
    }

    /// Moves `seen` to the front of the shared order, answering whether it is
    /// still the entry that was read: another cache may have evicted or
    /// replaced it in the meantime, and what was read is then not a hit.
    fn touch(&self, key: &str, seen: &Entry) -> Result<bool> {
        let _guard = self.lock(MANIFEST_LOCK_FILE)?;
        let mut manifest = self.read_manifest();
        let used = next_use(&manifest);
        let held = manifest
            .get_mut(key)
            .filter(|entry| entry.stored == seen.stored);
        let Some(entry) = held else {
            return Ok(false);
        };
        entry.used = used;
        self.write_manifest(&manifest)?;
        Ok(true)
    }

    fn discard(&self, key: &str, seen: &Entry) -> Result<()> {
        let _guard = self.lock(MANIFEST_LOCK_FILE)?;
        let mut manifest = self.read_manifest();
        // Another cache may have stored the key again between the lookup that
        // found it wanting and this lock, so only drop the entry that was.
        if manifest
            .get(key)
            .is_none_or(|entry| entry.stored != seen.stored)
        {
            return Ok(());
        }
        self.remove(&mut manifest, key);
        self.write_manifest(&manifest)
    }

    fn evict(&self, manifest: &mut Manifest, incoming: u64) {
        // The sizes come off the disk, so they are added the way the rest of the
        // manifest is read: a total that cannot be represented must leave the
        // cache evicting, not panicking in the middle of a container starting.
        let mut stored = manifest
            .values()
            .fold(0u64, |total, entry| total.saturating_add(entry.size));
        while stored.saturating_add(incoming) > self.config.max_bytes {
            let least_used = manifest
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, entry)| (key.clone(), entry.size));
            let Some((key, size)) = least_used else {
                break;
            };
            stored = stored.saturating_sub(size);
            self.remove(manifest, &key);
            self.counters.lock().unwrap().evictions += 1;
        }
    }

    fn remove(&self, manifest: &mut Manifest, key: &str) {
        manifest.remove(key);
        let _ = std::fs::remove_file(self.config.dir.join(key));
    }

    fn sweep(&self, manifest: &Manifest) -> Result<u64> {
        let mut swept = 0;
        for file in std::fs::read_dir(&self.config.dir)? {
            let file = file?;
            let name = file.file_name();
            // Everything this cache writes is named in ASCII, so a name it
            // cannot read back as such is one it never wrote.
            let ours = name.to_str().is_some_and(|name| {
                [MANIFEST_FILE, MANIFEST_LOCK_FILE, BUILD_LOCK_FILE].contains(&name)
                    || manifest.contains_key(name)
            });
            if ours || !file.file_type()?.is_file() {
                continue;
            }
            std::fs::remove_file(file.path())?;
            swept += 1;
        }
        Ok(swept)
    }

    fn lock(&self, name: &str) -> Result<File> {
        lock_file(&self.config.dir.join(name))
    }

    fn read_manifest(&self) -> Manifest {
        let mut manifest: Manifest = read_regular_file(&self.config.dir.join(MANIFEST_FILE))
            .ok()
            .and_then(|manifest| serde_json::from_slice(&manifest).ok())
            .unwrap_or_default();
        // The manifest is whatever is on disk. A key that this cache would not
        // have written can name a path of its own choosing, so it is dropped
        // here rather than joined onto the directory and opened or deleted.
        manifest.retain(|key, _| is_key(key));
        manifest
    }

    fn write_manifest(&self, manifest: &Manifest) -> Result<()> {
        atomic_write(
            &self.config.dir.join(MANIFEST_FILE),
            &serde_json::to_vec(manifest)?,
        )
    }

    fn absorb<T>(&self, result: Result<T>) -> Option<T> {
        result
            .inspect_err(|e| log::debug!("Module cache unusable, carrying on. Error: {e}"))
            .ok()
    }

    fn is_expired(&self, entry: &Entry) -> bool {
        Duration::from_nanos(now().saturating_sub(entry.stored)) >= self.config.ttl
    }
}

/// A [`Compiler`] that serves layers from a [`ModuleCache`], passing on only
/// the ones the cache cannot serve.
pub struct CachedCompiler<C> {
    inner: C,
    cache: Option<ModuleCache>,
}

impl<C> CachedCompiler<C> {
    /// Wraps `inner` with `cache`. A `None` cache passes every layer to `inner`.
    pub fn new(inner: C, cache: Option<ModuleCache>) -> Self {
        Self { inner, cache }
    }

    /// Wraps `inner` with the cache the environment asks for, if it asks for
    /// one. A cache that will not open is left out rather than reported: it
    /// would only cost the container the compilation it was meant to save.
    pub fn from_env(inner: C) -> Self {
        let cache = CacheConfig::from_env().and_then(|config| {
            ModuleCache::open(config)
                .inspect_err(|e| {
                    log::warn!("Error opening the module cache, continuing without it. Error: {e}")
                })
                .ok()
        });
        Self::new(inner, cache)
    }

    /// Returns the statistics of the cache in use, if there is one.
    pub fn stats(&self) -> Option<CacheStats> {
        self.cache.as_ref().map(ModuleCache::stats)
    }
}

impl<C: Compiler> CachedCompiler<C> {
    fn key(&self, layer: &WasmLayer) -> String {
        let mut hasher = DefaultHasher::new();
        self.inner.cache_key().hash(&mut hasher);
        format!("{}-{:x}", digest(layer.layer.as_slice()), hasher.finish())
    }
}

impl<C: Compiler> Compiler for CachedCompiler<C> {
    fn cache_key(&self) -> impl Hash {
        self.inner.cache_key()
    }

    async fn compile(&self, layers: &[WasmLayer]) -> Result<Vec<Option<Vec<u8>>>> {
        let Some(cache) = &self.cache else {
            return self.inner.compile(layers).await;
        };

        // A layer that is already precompiled has nothing to compile and nothing
        // worth keeping, so it never gets a key and stays out of the lookups, the
        // counters and the batch handed on. Its place in the answer is still its
        // own, which is why the keys are held per position rather than compacted.
        let keys: Vec<Option<String>> = layers
            .iter()
            .map(|layer| (!layer.precompiled).then(|| self.key(layer)))
            .collect();
        let mut first = HashMap::new();
        for (index, key) in keys.iter().enumerate() {
            if let Some(key) = key.as_deref() {
                first.entry(key).or_insert(index);
            }
        }
        let mut compiled: Vec<Option<Vec<u8>>> = keys
            .iter()
            .map(|key| key.as_deref().and_then(|key| cache.get(key)))
            .collect();
        // Which positions missed a lookup of their own. A repeat that was served
        // stays served: letting repeats follow the first appearance before this
        // is settled would send it back for a second look, and count a second
        // hit, only because an earlier position happened to miss.
        let missing: Vec<bool> = keys
            .iter()
            .zip(&compiled)
            .map(|(key, layer)| key.is_some() && layer.is_none())
            .collect();
        if !missing.contains(&true) {
            follow_first(&mut compiled, &keys, &first);
            return Ok(compiled);
        }

        let _building = cache.building().await;
        for index in 0..keys.len() {
            let Some(key) = keys[index].as_deref() else {
                continue;
            };
            if missing[index] {
                compiled[index] = cache.recheck(key);
            }
        }
        follow_first(&mut compiled, &keys, &first);

        let mut pending = Vec::new();
        let mut position = HashMap::new();
        for (index, key) in keys.iter().enumerate() {
            let Some(key) = key else {
                continue;
            };
            if compiled[index].is_some() {
                continue;
            }
            if let hash_map::Entry::Vacant(slot) = position.entry(key.as_str()) {
                slot.insert(pending.len());
                pending.push(index);
            }
        }
        if pending.is_empty() {
            return Ok(compiled);
        }

        let missed: Vec<_> = pending.iter().map(|&index| layers[index].clone()).collect();
        cache.counters.lock().unwrap().compilations += missed.len() as u64;
        let fresh = self.inner.compile(&missed).await?;
        if fresh.len() != missed.len() {
            bail!(
                "compiler returned {} layers for the {} layers it was given",
                fresh.len(),
                missed.len()
            );
        }

        for (&index, bytes) in pending.iter().zip(&fresh) {
            if let (Some(key), Some(bytes)) = (keys[index].as_deref(), bytes) {
                cache.put(key, bytes);
            }
        }
        for (slot, key) in compiled.iter_mut().zip(&keys) {
            let Some(key) = key.as_deref() else {
                continue;
            };
            if slot.is_none() {
                *slot = fresh[position[key]].clone();
            }
        }

        Ok(compiled)
    }
}

/// The precompiler a shim runs with: whatever it offers for `spec`, behind
/// whatever cache the environment asks for.
pub async fn precompiler<S: Shim>(spec: &Spec) -> Option<impl Compiler> {
    S::compiler_with_spec(spec)
        .await
        .map(CachedCompiler::from_env)
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| is_regular_file_metadata(&metadata))
}

fn is_regular_file_metadata(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    metadata.is_file()
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        // A replaced FIFO must not block before its type can be checked.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

        // Inspect the cache entry itself rather than opening a reparse target.
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    if !is_regular_file_metadata(&file.metadata()?) {
        bail!("cache entry is not a regular file");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

// Only the descriptor returned by exclusive creation receives bytes. A planted
// symlink or FIFO at a candidate name is never opened for writing.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT_PENDING: AtomicU64 = AtomicU64::new(0);
    for _ in 0..64 {
        let id = NEXT_PENDING.fetch_add(1, Ordering::Relaxed);
        let pending = path.with_extension(format!("{}-{id}{PENDING_SUFFIX}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = match options.open(&pending) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| {
            if !file.metadata()?.is_file() {
                bail!("cache temporary is not a regular file");
            }
            file.write_all(bytes)?;
            drop(file);
            std::fs::rename(&pending, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&pending);
        }
        return result;
    }
    bail!("could not create a cache temporary")
}

fn lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        bail!("cache lock is not a regular file");
    }
    file.lock()?;
    Ok(file)
}

fn parse_env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// Whether `key` is a name [`CachedCompiler::key`] would have produced: the
/// digest of a layer, a dash, and the hash of the wrapped compiler's cache key.
fn is_key(key: &str) -> bool {
    fn hex(text: &str) -> bool {
        text.bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    }

    let Some((layer, compiler)) = key.split_once('-') else {
        return false;
    };
    layer.len() == 64 && hex(layer) && (1..=16).contains(&compiler.len()) && hex(compiler)
}

/// Gives every repeated layer the answer its first appearance got. Each position
/// is looked up in its own right, but the directory can change between two
/// lookups of one layer, and the answer a repeat carries is not its own.
fn follow_first(
    compiled: &mut [Option<Vec<u8>>],
    keys: &[Option<String>],
    first: &HashMap<&str, usize>,
) {
    for index in 0..keys.len() {
        let Some(key) = keys[index].as_deref() else {
            continue;
        };
        let lead = first[key];
        if lead != index {
            compiled[index] = compiled[lead].clone();
        }
    }
}

fn next_use(manifest: &Manifest) -> u64 {
    manifest
        .values()
        .map(|entry| entry.used)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
