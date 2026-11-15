use std::hash::{DefaultHasher, Hash, Hasher as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use containerd_shim_wasm::sandbox::context::WasmLayer;
use containerd_shim_wasm::shim::Compiler;
use containerd_shim_wasm::shim::cache::{CacheConfig, CacheStats, CachedCompiler, ModuleCache};
use oci_spec::image::{Descriptor, Digest, MediaType};
use tempfile::TempDir;
use tokio::sync::Notify;

/// What a `FakeCompiler` records: the layers it was handed, per call.
type Calls = Arc<Mutex<Vec<Vec<Vec<u8>>>>>;

const HOUR: Duration = Duration::from_secs(60 * 60);
const BUDGET: u64 = 1024 * 1024;
const LAYER_SIZE: u64 = 12;
const WIDE_SIZE: u64 = 15;
const HOLD: Duration = Duration::from_millis(500);
const GRIP: Duration = Duration::from_secs(1);
const PATIENCE: Duration = Duration::from_secs(30);

const HELPER: &str = "the_directory_is_held_for_another_process";
const HELPER_CACHE: &str = "RUNWASI_CACHE_TEST_HELPER_CACHE";
const HELPER_MARKS: &str = "RUNWASI_CACHE_TEST_HELPER_MARKS";

/// The error `FakeCompiler::failing` gives back, so a test can tell the wrapped
/// compiler's own failure from one the wrapper invented in its place.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("compilation failed")
    }
}

impl std::error::Error for Refused {}

#[derive(Clone)]
struct FakeCompiler {
    key: String,
    calls: Calls,
    skipped: Vec<Vec<u8>>,
    error: bool,
    truncate: bool,
    lengthen: bool,
    started: Option<Arc<Notify>>,
    gate: Option<Arc<Notify>>,
    hold: Duration,
    log: Option<(Arc<Mutex<Vec<String>>>, &'static str)>,
    marks: Option<PathBuf>,
    watched: Option<(PathBuf, Arc<Mutex<Vec<bool>>>)>,
}

impl FakeCompiler {
    fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            calls: Arc::default(),
            skipped: Vec::new(),
            error: false,
            truncate: false,
            lengthen: false,
            started: None,
            gate: None,
            hold: Duration::ZERO,
            log: None,
            marks: None,
            watched: None,
        }
    }

    fn skipping(key: &str, skipped: &[&[u8]]) -> Self {
        Self {
            skipped: skipped.iter().map(|bytes| bytes.to_vec()).collect(),
            ..Self::new(key)
        }
    }

    fn failing() -> Self {
        Self {
            error: true,
            ..Self::new("v1")
        }
    }

    fn truncating() -> Self {
        Self {
            truncate: true,
            ..Self::new("v1")
        }
    }

    fn lengthening() -> Self {
        Self {
            lengthen: true,
            ..Self::new("v1")
        }
    }

    fn slow(key: &str, started: Arc<Notify>, hold: Duration) -> Self {
        Self {
            started: Some(started),
            hold,
            ..Self::new(key)
        }
    }

    /// Stays inside `compile` until `gate` is notified, so a test can decide
    /// what has to happen while a compilation is under way.
    fn gated(key: &str, started: Arc<Notify>, gate: Arc<Notify>) -> Self {
        Self {
            started: Some(started),
            gate: Some(gate),
            ..Self::new(key)
        }
    }

    /// Used in the helper process: it reports being inside the compiler, waits
    /// for the other process to reach the cache, and reports leaving.
    fn marking(marks: PathBuf) -> Self {
        Self {
            marks: Some(marks),
            ..Self::new("v1")
        }
    }

    /// Records, on every call, whether the helper process had already left its
    /// own compiler.
    fn watching(self, releasing: PathBuf) -> Self {
        Self {
            watched: Some((releasing, Arc::default())),
            ..self
        }
    }

    fn observed(&self) -> Vec<bool> {
        self.watched.as_ref().unwrap().1.lock().unwrap().clone()
    }

    fn logging(self, log: Arc<Mutex<Vec<String>>>, label: &'static str) -> Self {
        Self {
            log: Some((log, label)),
            ..self
        }
    }

    fn note(&self, what: &str) {
        if let Some((log, label)) = &self.log {
            log.lock().unwrap().push(format!("{label} {what}"));
        }
    }

    fn calls(&self) -> Vec<Vec<Vec<u8>>> {
        self.calls.lock().unwrap().clone()
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Compiler for FakeCompiler {
    fn cache_key(&self) -> impl Hash {
        self.key.clone()
    }

    async fn compile(&self, layers: &[WasmLayer]) -> Result<Vec<Option<Vec<u8>>>> {
        if self.error {
            return Err(Refused.into());
        }
        self.calls
            .lock()
            .unwrap()
            .push(layers.iter().map(|layer| layer.layer.clone()).collect());
        if let Some((releasing, seen)) = &self.watched {
            seen.lock().unwrap().push(releasing.exists());
        }
        if let Some(marks) = &self.marks {
            std::fs::write(marks.join("holding"), b"").unwrap();
            wait_for(&marks.join("ready")).await;
            tokio::time::sleep(GRIP).await;
            std::fs::write(marks.join("releasing"), b"").unwrap();
        }
        self.note("start");
        if let Some(started) = &self.started {
            started.notify_one();
        }
        if let Some(gate) = &self.gate {
            gate.notified().await;
        } else if !self.hold.is_zero() {
            tokio::time::sleep(self.hold).await;
        }
        let mut answers: Vec<_> = layers
            .iter()
            .map(|layer| (!self.skipped.contains(&layer.layer)).then(|| compiled(&layer.layer)))
            .collect();
        if self.truncate {
            answers.pop();
        }
        if self.lengthen {
            answers.push(Some(compiled(b"spare")));
        }
        self.note("end");
        Ok(answers)
    }
}

/// Captures nested test output without risking a full pipe blocking the child.
struct TestProcess {
    child: std::process::Child,
    output: tempfile::NamedTempFile,
}

impl TestProcess {
    fn spawn(command: &mut Command) -> Self {
        let output = tempfile::NamedTempFile::new().unwrap();
        let child = command
            .stdout(Stdio::from(output.as_file().try_clone().unwrap()))
            .stderr(Stdio::from(output.as_file().try_clone().unwrap()))
            .spawn()
            .expect("the helper process should start");
        Self { child, output }
    }

    fn diagnostics(&self) -> String {
        match std::fs::read(self.output.path()) {
            Ok(bytes) => String::from_utf8_lossy(&bytes)
                .lines()
                // Nested libtest result lines are diagnostics, not parent results.
                .map(|line| format!("    child: {line}\n"))
                .collect(),
            Err(error) => error.to_string(),
        }
    }

    fn wait_for(&mut self, marker: &Path) {
        let deadline = std::time::Instant::now() + PATIENCE;
        while !marker.exists() {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "helper exited before {marker:?}:\n{}",
                self.diagnostics()
            );
            assert!(
                std::time::Instant::now() < deadline,
                "helper never reached {marker:?}:\n{}",
                self.diagnostics()
            );
            sleep(Duration::from_millis(10));
        }
    }

    fn finish(&mut self) {
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "helper failed ({status}):\n{}",
                    self.diagnostics()
                );
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "helper did not finish: writable/readable cache failures must not hang compilation\n{}",
                self.diagnostics()
            );
            sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            eprintln!("nested test output:\n{}", self.diagnostics());
        }
    }
}

fn in_child_process(test: &str) -> bool {
    const CHILD: &str = "RUNWASI_CACHE_TEST_ISOLATED";
    if std::env::var(CHILD).ok().as_deref() == Some(test) {
        return true;
    }
    TestProcess::spawn(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--test-threads=1", "--nocapture"])
            .env(CHILD, test),
    )
    .finish();
    false
}

fn corrupt_cache_files(dir: &Path, fifo: bool) {
    use std::os::unix::ffi::OsStrExt;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_file() {
            continue;
        }
        if fifo {
            // Corrupt persisted data, leaving empty coordination files alone.
            if std::fs::metadata(&path).unwrap().len() == 0 {
                continue;
            }
            std::fs::remove_file(&path).unwrap();
            let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            // The CString remains alive for the call, and the path was removed above.
            let result = unsafe { libc::mkfifo(name.as_ptr(), 0o600) };
            assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        } else {
            std::fs::write(&path, b"not what the cache wrote").unwrap();
        }
    }
}

fn compiled(bytes: &[u8]) -> Vec<u8> {
    [b"compiled:".as_slice(), bytes].concat()
}

fn precompiled_layer(bytes: &[u8]) -> WasmLayer {
    WasmLayer {
        precompiled: true,
        ..layer(bytes)
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

/// The same contents wearing descriptor metadata that shares nothing with what
/// `layer` puts on them.
fn differently_described(bytes: &[u8]) -> WasmLayer {
    WasmLayer {
        config: Descriptor::new(
            MediaType::Other("application/vnd.module.wasm.content.layer.v1+wasm".to_string()),
            bytes.len() as u64 + 7,
            Digest::try_from(format!("sha256:{:064x}", 1)).unwrap(),
        ),
        ..layer(bytes)
    }
}

fn open(dir: &TempDir, ttl: Duration, max_bytes: u64) -> ModuleCache {
    open_at(dir.path().to_path_buf(), ttl, max_bytes)
}

fn open_at(dir: PathBuf, ttl: Duration, max_bytes: u64) -> ModuleCache {
    ModuleCache::open(CacheConfig {
        dir,
        ttl,
        max_bytes,
    })
    .expect("the cache should open")
}

fn compiler(dir: &TempDir, inner: FakeCompiler, max_bytes: u64) -> CachedCompiler<FakeCompiler> {
    CachedCompiler::new(inner, Some(open(dir, HOUR, max_bytes)))
}

fn stats(compiler: &CachedCompiler<FakeCompiler>) -> CacheStats {
    compiler.stats().expect("the compiler should have a cache")
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn stored_layer(dir: &TempDir, bytes: &[u8]) -> PathBuf {
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if std::fs::read(&path).is_ok_and(|found| found == bytes) {
            return path;
        }
    }
    panic!("no file in the cache holds the stored layer");
}

async fn wait_for(path: &Path) {
    let deadline = std::time::Instant::now() + PATIENCE;
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "{path:?} never appeared"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn block_on<F: std::future::Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(work)
}

fn with_cache_dir<R>(dir: Option<&str>, work: impl FnOnce() -> R) -> R {
    temp_env::with_vars(
        [
            ("RUNWASI_MODULE_CACHE_DIR", dir),
            ("RUNWASI_MODULE_CACHE_TTL_SECS", None),
            ("RUNWASI_MODULE_CACHE_MAX_BYTES", None),
        ],
        work,
    )
}

fn sleep_until(deadline: std::time::Instant) {
    sleep(deadline.saturating_duration_since(std::time::Instant::now()));
}

fn payload(task: u32, index: u32) -> Vec<u8> {
    format!("{task:03}{index:03}").into_bytes()
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_is_compiled_once_and_then_served_from_the_cache() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    let first = compiler.compile(&[layer(b"aaa")]).await.unwrap();
    let second = compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(first, vec![Some(compiled(b"aaa"))]);
    assert_eq!(second, first);
    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec()]]);
}

#[tokio::test(flavor = "current_thread")]
async fn the_statistics_count_the_hits_and_the_misses() {
    let dir = TempDir::new().unwrap();
    let compiler = compiler(&dir, FakeCompiler::new("v1"), BUDGET);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    assert_eq!(stats(&compiler).misses, 1);
    assert_eq!(stats(&compiler).hits, 0);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    assert_eq!(stats(&compiler).misses, 1);
    assert_eq!(stats(&compiler).hits, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn the_statistics_report_what_is_stored() {
    let dir = TempDir::new().unwrap();
    let compiler = compiler(&dir, FakeCompiler::new("v1"), BUDGET);

    compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    let stats = stats(&compiler);
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.bytes, 2 * LAYER_SIZE);
    assert_eq!(stats.compilations, 2);
    assert_eq!(stats.evictions, 0);
    assert_eq!(stats.expirations, 0);
    assert_eq!(stats.rejected, 0);
    assert_eq!(stats.swept, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn the_statistics_leave_out_a_layer_that_is_gone() {
    let dir = TempDir::new().unwrap();
    let compiler = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();
    assert_eq!(stats(&compiler).entries, 2);

    std::fs::remove_file(stored_layer(&dir, &compiled(b"aaa"))).unwrap();

    let stats = stats(&compiler);
    assert_eq!(stats.entries, 1);
    assert_eq!(stats.bytes, LAYER_SIZE);
}

#[tokio::test(flavor = "current_thread")]
async fn the_statistics_of_a_fresh_cache_are_empty() {
    let dir = TempDir::new().unwrap();

    assert_eq!(open(&dir, HOUR, BUDGET).stats(), CacheStats::default());
}

#[tokio::test(flavor = "current_thread")]
async fn only_the_layers_the_cache_cannot_serve_reach_the_compiler() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    compiler.compile(&[layer(b"bbb")]).await.unwrap();
    let compiled_layers = compiler
        .compile(&[layer(b"aaa"), layer(b"bbb"), layer(b"ccc")])
        .await
        .unwrap();

    assert_eq!(
        compiled_layers,
        vec![
            Some(compiled(b"aaa")),
            Some(compiled(b"bbb")),
            Some(compiled(b"ccc")),
        ]
    );
    assert_eq!(
        inner.calls(),
        vec![
            vec![b"bbb".to_vec()],
            vec![b"aaa".to_vec(), b"ccc".to_vec()],
        ]
    );
    assert_eq!(stats(&compiler).compilations, 3);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_repeated_in_one_call_is_compiled_once() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    let compiled_layers = compiler
        .compile(&[layer(b"aaa"), layer(b"bbb"), layer(b"aaa")])
        .await
        .unwrap();

    assert_eq!(
        compiled_layers,
        vec![
            Some(compiled(b"aaa")),
            Some(compiled(b"bbb")),
            Some(compiled(b"aaa")),
        ]
    );
    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec(), b"bbb".to_vec()]]);

    let stats = stats(&compiler);
    assert_eq!(stats.compilations, 2);
    assert_eq!(stats.misses, 3);
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.bytes, 2 * LAYER_SIZE);

    #[derive(Clone, Default)]
    struct ChangingCompiler(Arc<Mutex<usize>>);
    impl Compiler for ChangingCompiler {
        fn cache_key(&self) -> impl Hash {
            "changing-output"
        }
        async fn compile(&self, layers: &[WasmLayer]) -> Result<Vec<Option<Vec<u8>>>> {
            assert_eq!(layers.len(), 1, "only one distinct input is submitted");
            let mut generation = self.0.lock().unwrap();
            *generation += 1;
            Ok(vec![Some(vec![*generation as u8])])
        }
    }

    // A repeated position must follow its first appearance even if expiration
    // changes the directory during the batch. The invariant is the same whether
    // the first lookup hits or misses; no exact scheduling or hit count is required.
    for ttl in [10, 50, 200].map(Duration::from_millis) {
        let dir = TempDir::new().unwrap();
        let inner = ChangingCompiler::default();
        let cached = CachedCompiler::new(inner.clone(), Some(open(&dir, ttl, BUDGET)));
        cached.compile(&[layer(b"repeated")]).await.unwrap();
        let repeated = vec![layer(b"repeated"); 2048];
        let answers = cached.compile(&repeated).await.unwrap();
        assert_eq!(answers.len(), repeated.len());
        let first = answers[0].clone();
        assert!(first.is_some());
        for (position, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer, &first,
                "position {position} did not follow its first appearance"
            );
        }
        if first == Some(vec![1]) {
            assert_eq!(
                *inner.0.lock().unwrap(),
                1,
                "a served repeat must not be recompiled"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_repeated_across_calls_is_served_from_the_cache() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    compiler
        .compile(&[layer(b"aaa"), layer(b"aaa")])
        .await
        .unwrap();
    compiler
        .compile(&[layer(b"aaa"), layer(b"aaa")])
        .await
        .unwrap();

    assert_eq!(inner.call_count(), 1);
    let stats = stats(&compiler);
    assert_eq!(stats.hits, 2);
    assert_eq!(stats.misses, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn layers_with_different_contents_are_cached_apart() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"bbb")]).await.unwrap();
    let compiled_layers = compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    assert_eq!(
        compiled_layers,
        vec![Some(compiled(b"aaa")), Some(compiled(b"bbb"))]
    );
    assert_eq!(inner.call_count(), 2);
    assert_eq!(stats(&compiler).entries, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_precompiled_layer_is_left_alone() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    let answered = compiler
        .compile(&[precompiled_layer(b"aaa")])
        .await
        .unwrap();

    assert_eq!(answered, vec![None]);
    assert_eq!(inner.call_count(), 0);
    assert_eq!(stats(&compiler), CacheStats::default());
}

#[tokio::test(flavor = "current_thread")]
async fn a_precompiled_layer_keeps_the_others_in_place() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    let answered = compiler
        .compile(&[
            precompiled_layer(b"xxx"),
            layer(b"aaa"),
            precompiled_layer(b"yyy"),
            layer(b"bbb"),
        ])
        .await
        .unwrap();

    assert_eq!(
        answered,
        vec![None, Some(compiled(b"aaa")), None, Some(compiled(b"bbb"))]
    );
    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec(), b"bbb".to_vec()]]);
    let stats = stats(&compiler);
    assert_eq!(stats.compilations, 2);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.entries, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_precompiled_layer_neither_fills_nor_reads_the_cache() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    compiler
        .compile(&[precompiled_layer(b"aaa")])
        .await
        .unwrap();
    assert_eq!(stats(&compiler).entries, 0);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    let answered = compiler
        .compile(&[precompiled_layer(b"aaa")])
        .await
        .unwrap();

    assert_eq!(answered, vec![None]);
    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec()]]);
    let stats = stats(&compiler);
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.entries, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_the_compiler_leaves_alone_is_not_stored() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::skipping("v1", &[b"aaa"]);
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    let first = compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(first, vec![None, Some(compiled(b"bbb"))]);
    assert_eq!(inner.call_count(), 2);
    assert_eq!(stats(&compiler).entries, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_repeated_layer_the_compiler_declines_is_declined_everywhere() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::skipping("v1", &[b"aaa"]);
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    let answered = compiler
        .compile(&[layer(b"aaa"), layer(b"aaa")])
        .await
        .unwrap();

    assert_eq!(answered, vec![None, None]);
    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec()]]);
    let stats = stats(&compiler);
    assert_eq!(stats.compilations, 1);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.entries, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_given_both_precompiled_and_not_is_compiled_once() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    let answered = compiler
        .compile(&[precompiled_layer(b"aaa"), layer(b"aaa")])
        .await
        .unwrap();

    assert_eq!(answered, vec![None, Some(compiled(b"aaa"))]);
    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec()]]);
    let stats = stats(&compiler);
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.compilations, 1);
    assert_eq!(stats.entries, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_precompiled_layer_after_the_same_one_keeps_its_own_answer() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    let answered = compiler
        .compile(&[layer(b"aaa"), precompiled_layer(b"aaa")])
        .await
        .unwrap();

    assert_eq!(answered, vec![Some(compiled(b"aaa")), None]);
    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec()]]);
    let stats = stats(&compiler);
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.compilations, 1);
    assert_eq!(stats.entries, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_list_of_layers_does_not_reach_the_compiler() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    assert!(compiler.compile(&[]).await.unwrap().is_empty());
    assert_eq!(inner.call_count(), 0);
    assert_eq!(stats(&compiler).compilations, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_compilation_error_is_passed_on() {
    let dir = TempDir::new().unwrap();
    let compiler = compiler(&dir, FakeCompiler::failing(), BUDGET);

    let failure = compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .expect_err("the compiler refused");

    assert!(
        failure.downcast_ref::<Refused>().is_some(),
        "the compiler's own error is what comes back, not one put in its place: {failure}"
    );
    let stats = stats(&compiler);
    assert_eq!(stats.entries, 0, "an error stores nothing");
    assert_eq!(
        stats.compilations, 2,
        "both layers went to the compiler, whatever it answered"
    );
    assert_eq!(stats.misses, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_of_the_wrong_length_from_the_compiler_is_an_error() {
    let short = TempDir::new().unwrap();
    let long = TempDir::new().unwrap();
    let truncating = compiler(&short, FakeCompiler::truncating(), BUDGET);
    let lengthening = compiler(&long, FakeCompiler::lengthening(), BUDGET);

    assert!(
        truncating
            .compile(&[layer(b"aaa"), layer(b"bbb")])
            .await
            .is_err()
    );
    assert_eq!(stats(&truncating).entries, 0);

    assert!(lengthening.compile(&[layer(b"aaa")]).await.is_err());
    assert_eq!(stats(&lengthening).entries, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_is_measured_against_what_the_compiler_was_given() {
    // one layer is already held, so the compiler is handed fewer layers than the
    // call carried: an answer sized to the call is still the wrong length
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling.compile(&[layer(b"aaa")]).await.unwrap();
    let lengthening = compiler(&dir, FakeCompiler::lengthening(), BUDGET);

    let answered = lengthening.compile(&[layer(b"aaa"), layer(b"bbb")]).await;

    assert!(
        answered.is_err(),
        "two answers for the one layer it was given"
    );
    assert_eq!(
        stats(&lengthening).entries,
        1,
        "and nothing more was stored"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_cache_key_recompiles_the_layer() {
    let dir = TempDir::new().unwrap();
    let first = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    first.compile(&[layer(b"aaa")]).await.unwrap();

    let inner = FakeCompiler::new("v2");
    let second = compiler(&dir, inner.clone(), BUDGET);
    second.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec()]]);
    assert_eq!(stats(&second).entries, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn the_cache_outlives_the_compiler_that_filled_it() {
    let dir = TempDir::new().unwrap();
    let first = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    first.compile(&[layer(b"aaa")]).await.unwrap();

    let inner = FakeCompiler::new("v1");
    let second = compiler(&dir, inner.clone(), BUDGET);
    let compiled_layers = second.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(compiled_layers, vec![Some(compiled(b"aaa"))]);
    assert_eq!(inner.call_count(), 0);
    assert_eq!(stats(&second).hits, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn an_open_cache_serves_what_another_one_stored_after_it_opened() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let waiting = compiler(&dir, inner.clone(), BUDGET);
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);

    filling.compile(&[layer(b"aaa")]).await.unwrap();
    let compiled_layers = waiting.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(compiled_layers, vec![Some(compiled(b"aaa"))]);
    assert_eq!(inner.call_count(), 0);
    assert_eq!(stats(&waiting).hits, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn the_statistics_follow_the_directory() {
    let dir = TempDir::new().unwrap();
    let watching = open(&dir, HOUR, BUDGET);
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);

    filling
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    let stats = watching.stats();
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.bytes, 2 * LAYER_SIZE);
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn entries_stored_by_two_caches_both_survive() {
    let dir = TempDir::new().unwrap();
    let first = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    let second = compiler(&dir, FakeCompiler::new("v1"), BUDGET);

    first.compile(&[layer(b"aaa")]).await.unwrap();
    second.compile(&[layer(b"bbb")]).await.unwrap();
    first.compile(&[layer(b"ccc")]).await.unwrap();

    let stats = open(&dir, HOUR, BUDGET).stats();
    assert_eq!(stats.entries, 3);
    assert_eq!(stats.bytes, 3 * LAYER_SIZE);
}

#[tokio::test(flavor = "current_thread")]
async fn an_entry_within_the_ttl_survives_reopening() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling.compile(&[layer(b"aaa")]).await.unwrap();

    let reopened = open(&dir, HOUR, BUDGET).stats();

    assert_eq!(reopened.entries, 1);
    assert_eq!(reopened.bytes, LAYER_SIZE);
    assert_eq!(reopened.expirations, 0);
    assert_eq!(reopened.swept, 0);
    assert_eq!(reopened.hits, 0);
    assert_eq!(reopened.misses, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn an_entry_that_outlived_the_ttl_is_recompiled() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let ttl = Duration::from_millis(50);
    let compiler = CachedCompiler::new(inner.clone(), Some(open(&dir, ttl, BUDGET)));

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    sleep(Duration::from_millis(250));
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.call_count(), 2);
    let stats = stats(&compiler);
    assert_eq!(stats.expirations, 1);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.entries, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_zero_ttl_leaves_nothing_usable() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = CachedCompiler::new(inner.clone(), Some(open(&dir, Duration::ZERO, BUDGET)));

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.call_count(), 2);
    let stats = stats(&compiler);
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.expirations, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn entries_that_outlived_the_ttl_are_dropped_when_the_cache_is_opened() {
    let dir = TempDir::new().unwrap();
    let ttl = Duration::from_millis(50);
    let compiler = CachedCompiler::new(FakeCompiler::new("v1"), Some(open(&dir, ttl, BUDGET)));
    compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    sleep(Duration::from_millis(250));
    let reopened = open(&dir, ttl, BUDGET).stats();

    assert_eq!(reopened.expirations, 2);
    assert_eq!(reopened.swept, 0);
    assert_eq!(reopened.entries, 0);
    assert_eq!(reopened.bytes, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn storing_a_layer_evicts_the_least_recently_used_entry() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), 2 * LAYER_SIZE);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"bbb")]).await.unwrap();
    compiler.compile(&[layer(b"ccc")]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.call_count(), 4);
    let stats = stats(&compiler);
    assert_eq!(stats.evictions, 2);
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.bytes, 2 * LAYER_SIZE);
}

#[tokio::test(flavor = "current_thread")]
async fn eviction_follows_use_and_not_size() {
    // three layers of three different sizes, with the middle one left longest
    // unused: evicting the smallest, the largest, or the oldest stored each
    // leaves a different set behind than evicting the least recently used
    let dir = TempDir::new().unwrap();
    let mid = b"midlay";
    let large = b"a layer of its own";
    let large_size = WIDE_SIZE + 12;
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), LAYER_SIZE + WIDE_SIZE + large_size);

    compiler.compile(&[layer(large)]).await.unwrap();
    compiler.compile(&[layer(mid)]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(large)]).await.unwrap();
    assert_eq!(
        inner.call_count(),
        3,
        "the wide layer was served, not compiled"
    );

    compiler.compile(&[layer(b"bbb")]).await.unwrap();

    let stats = stats(&compiler);
    assert_eq!(stats.evictions, 1, "one entry made enough room");
    assert_eq!(stats.entries, 3);
    assert_eq!(
        stats.bytes,
        large_size + LAYER_SIZE + LAYER_SIZE,
        "the entry that went was the least recently used one, not the smallest,          the largest, or the one stored first"
    );

    compiler.compile(&[layer(large)]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    assert_eq!(inner.call_count(), 4, "both of those are still held");
    compiler.compile(&[layer(mid)]).await.unwrap();
    assert_eq!(
        inner.call_count(),
        5,
        "the one that went had to be compiled again"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serving_a_layer_makes_it_the_most_recently_used() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), 2 * LAYER_SIZE);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"bbb")]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"ccc")]).await.unwrap();

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    assert_eq!(inner.call_count(), 3);

    compiler.compile(&[layer(b"bbb")]).await.unwrap();
    assert_eq!(inner.call_count(), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn a_refresh_by_one_cache_is_seen_by_another() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), 2 * LAYER_SIZE);
    let reading = compiler(&dir, FakeCompiler::new("v1"), 2 * LAYER_SIZE);

    filling.compile(&[layer(b"aaa")]).await.unwrap();
    filling.compile(&[layer(b"bbb")]).await.unwrap();
    reading.compile(&[layer(b"aaa")]).await.unwrap();
    filling.compile(&[layer(b"ccc")]).await.unwrap();

    let inner = FakeCompiler::new("v1");
    let checking = compiler(&dir, inner.clone(), 2 * LAYER_SIZE);
    checking.compile(&[layer(b"aaa")]).await.unwrap();
    assert_eq!(inner.call_count(), 0);

    checking.compile(&[layer(b"bbb")]).await.unwrap();
    assert_eq!(inner.calls(), vec![vec![b"bbb".to_vec()]]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_larger_than_the_size_bound_is_not_stored() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), LAYER_SIZE - 1);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.call_count(), 2);
    let stats = stats(&compiler);
    assert_eq!(stats.entries, 0);
    assert_eq!(stats.evictions, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_the_size_of_the_bound_is_stored() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), LAYER_SIZE);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.call_count(), 1);
    let stats = stats(&compiler);
    assert_eq!(stats.entries, 1);
    assert_eq!(stats.bytes, LAYER_SIZE);
    assert_eq!(stats.evictions, 0);
    assert_eq!(stats.hits, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn layers_that_exactly_fill_the_bound_evict_nothing() {
    let dir = TempDir::new().unwrap();
    let compiler = compiler(&dir, FakeCompiler::new("v1"), 2 * LAYER_SIZE);

    compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    let stats = stats(&compiler);
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.bytes, 2 * LAYER_SIZE);
    assert_eq!(stats.evictions, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_bound_of_zero_keeps_nothing() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), 0);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.call_count(), 2);
    let stats = stats(&compiler);
    assert_eq!(stats.entries, 0);
    assert_eq!(stats.bytes, 0);
    assert_eq!(stats.evictions, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn an_oversized_layer_leaves_the_stored_ones_alone() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), LAYER_SIZE);
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    let compiled_layers = compiler.compile(&[layer(b"a longer layer")]).await.unwrap();

    assert_eq!(compiled_layers, vec![Some(compiled(b"a longer layer"))]);
    let stats = stats(&compiler);
    assert_eq!(stats.entries, 1);
    assert_eq!(stats.bytes, LAYER_SIZE);
    assert_eq!(stats.evictions, 0);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    assert_eq!(inner.call_count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn one_call_may_store_more_than_the_bound_holds() {
    let dir = TempDir::new().unwrap();
    let compiler = compiler(&dir, FakeCompiler::new("v1"), 2 * LAYER_SIZE);

    let compiled_layers = compiler
        .compile(&[layer(b"aaa"), layer(b"bbb"), layer(b"ccc")])
        .await
        .unwrap();

    assert_eq!(
        compiled_layers,
        vec![
            Some(compiled(b"aaa")),
            Some(compiled(b"bbb")),
            Some(compiled(b"ccc")),
        ]
    );
    let stats = stats(&compiler);
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.evictions, 1);
    assert_eq!(stats.bytes, 2 * LAYER_SIZE);
}

#[tokio::test(flavor = "current_thread")]
async fn opening_brings_the_stored_layers_inside_a_lowered_bound() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    for bytes in [b"aaa", b"bbb", b"ccc", b"ddd"] {
        filling.compile(&[layer(bytes)]).await.unwrap();
    }
    filling.compile(&[layer(b"aaa")]).await.unwrap();
    drop(filling);

    let reopened = open(&dir, HOUR, LAYER_SIZE);
    let observed = reopened.stats();
    assert_eq!(observed.evictions, 3);
    assert_eq!(observed.entries, 1);
    assert_eq!(observed.bytes, LAYER_SIZE);
    assert_eq!(observed.swept, 0);
    drop(reopened);

    let inner = FakeCompiler::new("v1");
    let after = compiler(&dir, inner.clone(), LAYER_SIZE);
    assert_eq!(
        after.compile(&[layer(b"aaa")]).await.unwrap(),
        vec![Some(compiled(b"aaa"))]
    );
    assert_eq!(
        inner.call_count(),
        0,
        "the most recently used entry survives"
    );
    assert_eq!(
        after
            .compile(&[layer(b"bbb"), layer(b"ccc"), layer(b"ddd")])
            .await
            .unwrap(),
        vec![
            Some(compiled(b"bbb")),
            Some(compiled(b"ccc")),
            Some(compiled(b"ddd")),
        ]
    );
    assert_eq!(
        inner.calls(),
        vec![vec![b"bbb".to_vec(), b"ccc".to_vec(), b"ddd".to_vec()]]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_reconciled_directory_has_nothing_left_to_reconcile() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling.compile(&[layer(b"aaa")]).await.unwrap();
    filling.compile(&[layer(b"bbb")]).await.unwrap();
    filling.compile(&[layer(b"ccc")]).await.unwrap();

    let first = open(&dir, HOUR, 2 * LAYER_SIZE).stats();
    assert_eq!(first.evictions, 1);

    let again = open(&dir, HOUR, 2 * LAYER_SIZE).stats();

    assert_eq!(
        again.evictions, 0,
        "what the first opening dropped stayed dropped"
    );
    assert_eq!(
        again.rejected, 0,
        "no entry is left behind pointing at a layer the first opening removed"
    );
    assert_eq!(again.expirations, 0);
    assert_eq!(again.entries, 2);
    assert_eq!(again.bytes, 2 * LAYER_SIZE);
}

#[tokio::test(flavor = "current_thread")]
async fn a_stored_layer_is_a_file_holding_its_bytes() {
    let dir = TempDir::new().unwrap();
    let compiler = compiler(&dir, FakeCompiler::new("v1"), BUDGET);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    let path = stored_layer(&dir, &compiled(b"aaa"));
    assert_eq!(std::fs::read(path).unwrap(), compiled(b"aaa"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_that_changed_on_disk_is_not_served() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    let path = stored_layer(&dir, &compiled(b"aaa"));
    let mut changed = compiled(b"aaa");
    changed[0] ^= 0xff;
    std::fs::write(&path, &changed).unwrap();

    let compiled_layers = compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(compiled_layers, vec![Some(compiled(b"aaa"))]);
    assert_eq!(inner.call_count(), 2);
    let stats = stats(&compiler);
    assert_eq!(stats.rejected, 1);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.entries, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_repaired_after_a_rejection_is_served_again() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    let path = stored_layer(&dir, &compiled(b"aaa"));
    std::fs::write(&path, b"000000000000").unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.call_count(), 2);
    assert_eq!(stats(&compiler).hits, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_that_vanished_from_disk_is_compiled_again() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    std::fs::remove_file(stored_layer(&dir, &compiled(b"aaa"))).unwrap();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(inner.call_count(), 2);
    let stats = stats(&compiler);
    assert_eq!(stats.rejected, 1);
    assert_eq!(stats.entries, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn files_belonging_to_no_entry_are_swept_when_the_cache_is_opened() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling.compile(&[layer(b"aaa")]).await.unwrap();

    let stray = dir.path().join("belongs-to-no-entry");
    std::fs::write(&stray, b"left behind").unwrap();
    let reopened = open(&dir, HOUR, BUDGET);

    assert!(!stray.exists());
    let stats = reopened.stats();
    assert_eq!(stats.swept, 1);
    assert_eq!(stats.entries, 1);
    assert_eq!(stats.bytes, LAYER_SIZE);
}

#[tokio::test(flavor = "current_thread")]
async fn files_the_cache_never_wrote_are_swept_whatever_they_are_called() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling.compile(&[layer(b"aaa")]).await.unwrap();

    #[cfg(unix)]
    let named_outside_utf8 = {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        vec![dir.path().join(OsString::from_vec(vec![0xff, 0xfe]))]
    };
    #[cfg(not(unix))]
    let named_outside_utf8: Vec<PathBuf> = Vec::new();

    let strays: Vec<PathBuf> = [
        dir.path().join("belongs-to-no-entry"),
        dir.path().join(".hidden-and-unowned"),
        dir.path().join(".layer-deadbeef.bin.tmp-1234"),
    ]
    .into_iter()
    .chain(named_outside_utf8)
    .collect();
    for stray in &strays {
        std::fs::write(stray, b"left behind").unwrap();
    }

    let reopened = open(&dir, HOUR, BUDGET);

    for stray in &strays {
        assert!(!stray.exists(), "{stray:?} was left behind");
    }
    let stats = reopened.stats();
    assert_eq!(stats.swept, strays.len() as u64);
    assert_eq!(stats.entries, 1, "the layer an entry refers to stays");
    assert_eq!(stats.bytes, LAYER_SIZE);
}

#[tokio::test(flavor = "current_thread")]
async fn sweeping_keeps_the_layers_the_entries_refer_to() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling.compile(&[layer(b"aaa")]).await.unwrap();
    std::fs::write(dir.path().join("belongs-to-no-entry"), b"left behind").unwrap();

    let inner = FakeCompiler::new("v1");
    let after = compiler(&dir, inner.clone(), BUDGET);
    let served = after.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(served, vec![Some(compiled(b"aaa"))]);
    assert_eq!(inner.call_count(), 0);
    assert_eq!(stats(&after).swept, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn entries_whose_layer_is_gone_are_dropped_when_the_cache_is_opened() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();
    std::fs::remove_file(stored_layer(&dir, &compiled(b"aaa"))).unwrap();

    let reopened = open(&dir, HOUR, BUDGET).stats();

    assert_eq!(reopened.rejected, 1);
    assert_eq!(reopened.entries, 1);
    assert_eq!(reopened.bytes, LAYER_SIZE);
    assert_eq!(reopened.swept, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_replaced_by_a_directory_is_not_served() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    let stored = stored_layer(&dir, &compiled(b"aaa"));
    std::fs::remove_file(&stored).unwrap();
    std::fs::create_dir(&stored).unwrap();
    let answered = compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(answered, vec![Some(compiled(b"aaa"))]);
    assert_eq!(inner.call_count(), 2);
    let stats = stats(&compiler);
    assert_eq!(stats.rejected, 1);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.hits, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_the_cache_cannot_write_over_still_answers() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    // Obstruct the previous stored file. A replacement may use another path.
    let stored = stored_layer(&dir, &compiled(b"aaa"));
    std::fs::remove_file(&stored).unwrap();
    std::fs::create_dir(&stored).unwrap();
    std::fs::write(stored.join("in the way"), b"occupied").unwrap();

    for _ in 0..3 {
        let answered = compiler.compile(&[layer(b"aaa")]).await.unwrap();
        assert_eq!(answered, vec![Some(compiled(b"aaa"))]);
    }

    let calls = inner.call_count() as u64;
    assert!(
        (2..=4).contains(&calls),
        "the unusable layer was recompiled"
    );
    let stats = stats(&compiler);
    assert_eq!(stats.misses, calls);
    assert_eq!(stats.hits, 4 - calls);
    if stats.hits > 0 {
        stored_layer(&dir, &compiled(b"aaa"));
    }
    // whether the unusable record is dropped or kept and refused again is the
    // cache's own business; that it noticed at all is not
    assert!(
        (1..=4).contains(&stats.rejected),
        "the layer that would not read back was rejected: {stats:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_cache_opened_on_unreadable_bookkeeping_starts_empty() {
    if !in_child_process("a_cache_opened_on_unreadable_bookkeeping_starts_empty") {
        return;
    }
    for fifo in [false, true] {
        let dir = TempDir::new().unwrap();
        let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
        filling.compile(&[layer(b"aaa")]).await.unwrap();

        corrupt_cache_files(dir.path(), fifo);

        let inner = FakeCompiler::new("v1");
        let reopened = compiler(&dir, inner.clone(), BUDGET);

        assert_eq!(
            stats(&reopened).entries,
            0,
            "nothing the directory claims to hold is trusted"
        );
        let answered = reopened.compile(&[layer(b"aaa")]).await.unwrap();
        assert_eq!(answered, vec![Some(compiled(b"aaa"))]);
        assert_eq!(inner.call_count(), 1);

        let again = reopened.compile(&[layer(b"aaa")]).await.unwrap();
        assert_eq!(again, answered, "and it keeps layers again from there");
        if !fifo {
            assert_eq!(inner.call_count(), 1);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_cache_whose_bookkeeping_is_unreadable_still_answers() {
    if !in_child_process("a_cache_whose_bookkeeping_is_unreadable_still_answers") {
        return;
    }
    for fifo in [false, true] {
        let dir = TempDir::new().unwrap();
        let inner = FakeCompiler::new("v1");
        let compiler = compiler(&dir, inner.clone(), BUDGET);
        compiler.compile(&[layer(b"aaa")]).await.unwrap();

        corrupt_cache_files(dir.path(), fifo);
        let answered = compiler
            .compile(&[layer(b"aaa"), layer(b"bbb")])
            .await
            .unwrap();

        assert_eq!(
            answered,
            vec![Some(compiled(b"aaa")), Some(compiled(b"bbb"))]
        );
        assert_eq!(inner.call_count(), 2);
        stats(&compiler);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_cache_whose_directory_is_gone_still_answers() {
    let dir = TempDir::new().unwrap();
    let cache_dir = dir.path().join("cache");
    let inner = FakeCompiler::new("v1");
    let compiler = CachedCompiler::new(
        inner.clone(),
        Some(open_at(cache_dir.clone(), HOUR, BUDGET)),
    );
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    std::fs::remove_dir_all(&cache_dir).unwrap();
    let compiled_layers = compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    assert_eq!(
        compiled_layers,
        vec![Some(compiled(b"aaa")), Some(compiled(b"bbb"))]
    );
    assert_eq!(inner.call_count(), 2);
    let gone = stats(&compiler);
    assert_eq!(gone.entries, 0);
    assert_eq!(gone.bytes, 0);
    assert_eq!(gone.misses, 3);

    std::fs::write(&cache_dir, b"not a directory").unwrap();
    let served = compiler.compile(&[layer(b"ccc")]).await.unwrap();

    assert_eq!(served, vec![Some(compiled(b"ccc"))]);
    assert_eq!(inner.call_count(), 3);
    assert_eq!(stats(&compiler).entries, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn the_cache_directory_is_created_when_it_does_not_exist() {
    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("precompiled").join("wasmtime");

    let cache = open_at(nested.clone(), HOUR, BUDGET);

    assert!(nested.is_dir());
    assert_eq!(cache.stats(), CacheStats::default());

    let occupied = dir.path().join("occupied");
    std::fs::write(&occupied, b"not a directory").unwrap();
    assert!(
        ModuleCache::open(CacheConfig {
            dir: occupied.join("cache"),
            ttl: HOUR,
            max_bytes: BUDGET,
        })
        .is_err()
    );
}

#[test]
fn the_statistics_are_nine_u64_fields_that_all_print() {
    fn takes_u64(_: u64) {}

    let counts = CacheStats::default();
    takes_u64(counts.entries);
    takes_u64(counts.bytes);
    takes_u64(counts.hits);
    takes_u64(counts.misses);
    takes_u64(counts.evictions);
    takes_u64(counts.expirations);
    takes_u64(counts.rejected);
    takes_u64(counts.swept);
    takes_u64(counts.compilations);

    let empty = format!("{:?}", CacheStats::default());
    for field in [
        "entries",
        "bytes",
        "hits",
        "misses",
        "evictions",
        "expirations",
        "rejected",
        "swept",
        "compilations",
    ] {
        assert!(empty.contains(field), "{field} is not shown: {empty}");
    }

    let filled = [
        CacheStats {
            entries: 1,
            ..Default::default()
        },
        CacheStats {
            bytes: 1,
            ..Default::default()
        },
        CacheStats {
            hits: 1,
            ..Default::default()
        },
        CacheStats {
            misses: 1,
            ..Default::default()
        },
        CacheStats {
            evictions: 1,
            ..Default::default()
        },
        CacheStats {
            expirations: 1,
            ..Default::default()
        },
        CacheStats {
            rejected: 1,
            ..Default::default()
        },
        CacheStats {
            swept: 1,
            ..Default::default()
        },
        CacheStats {
            compilations: 1,
            ..Default::default()
        },
    ];
    for stats in filled {
        assert_ne!(format!("{stats:?}"), empty, "a field is left unprinted");
    }
}

#[test]
fn the_environment_turns_the_cache_on_for_a_compiler() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");

    with_cache_dir(dir.path().to_str(), || {
        let compiler = CachedCompiler::from_env(inner.clone());
        block_on(async {
            compiler.compile(&[layer(b"aaa")]).await.unwrap();
            compiler.compile(&[layer(b"aaa")]).await.unwrap();
        });

        assert_eq!(inner.call_count(), 1);
        assert_eq!(compiler.stats().map(|stats| stats.hits), Some(1));
    });
}

#[test]
fn a_cache_that_will_not_open_leaves_the_compiler_uncached() {
    let dir = TempDir::new().unwrap();
    let occupied = dir.path().join("occupied");
    std::fs::write(&occupied, b"not a directory").unwrap();
    let inner = FakeCompiler::new("v1");

    with_cache_dir(occupied.join("cache").to_str(), || {
        let compiler = CachedCompiler::from_env(inner.clone());
        assert!(compiler.stats().is_none());
        block_on(async {
            compiler.compile(&[layer(b"aaa")]).await.unwrap();
            compiler.compile(&[layer(b"aaa")]).await.unwrap();
        });

        assert_eq!(inner.call_count(), 2);
    });
}

#[test]
fn an_environment_without_a_directory_leaves_the_compiler_uncached() {
    let inner = FakeCompiler::new("v1");

    with_cache_dir(None, || {
        let compiler = CachedCompiler::from_env(inner.clone());
        assert!(compiler.stats().is_none());
        block_on(async {
            compiler.compile(&[layer(b"aaa")]).await.unwrap();
            compiler.compile(&[layer(b"aaa")]).await.unwrap();
        });

        assert_eq!(inner.call_count(), 2);
    });
}

#[tokio::test(flavor = "current_thread")]
async fn a_compiler_without_a_cache_has_no_statistics() {
    let compiler = CachedCompiler::new(FakeCompiler::new("v1"), None);

    assert!(compiler.stats().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_compiler_without_a_cache_passes_every_layer_on() {
    let inner = FakeCompiler::new("v1");
    let compiler = CachedCompiler::new(inner.clone(), None);

    let first = compiler
        .compile(&[layer(b"aaa"), layer(b"aaa")])
        .await
        .unwrap();
    let second = compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(first, vec![Some(compiled(b"aaa")), Some(compiled(b"aaa"))]);
    assert_eq!(second, vec![Some(compiled(b"aaa"))]);
    assert_eq!(
        inner.calls(),
        vec![
            vec![b"aaa".to_vec(), b"aaa".to_vec()],
            vec![b"aaa".to_vec()],
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_cache_key_of_the_wrapped_compiler_is_kept() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let uncached = CachedCompiler::new(inner.clone(), None);
    let cached = compiler(&dir, inner.clone(), BUDGET);

    for compiler in [&uncached, &cached] {
        assert_eq!(hash_of(compiler.cache_key()), hash_of(inner.cache_key()));
        assert_ne!(
            hash_of(compiler.cache_key()),
            hash_of(FakeCompiler::new("v2").cache_key())
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_cache_waiting_for_a_compilation_leaves_the_runtime_free() {
    let dir = TempDir::new().unwrap();
    let started = Arc::new(Notify::new());
    let slow = FakeCompiler::slow("v1", started.clone(), HOLD);
    let quick = FakeCompiler::new("v1");
    let building = compiler(&dir, slow.clone(), BUDGET);
    let waiting = compiler(&dir, quick.clone(), BUDGET);

    let layers = [layer(b"aaa")];
    let (built, served) = tokio::join!(building.compile(&layers), async {
        started.notified().await;
        waiting.compile(&layers).await
    });

    assert_eq!(built.unwrap(), vec![Some(compiled(b"aaa"))]);
    assert_eq!(served.unwrap(), vec![Some(compiled(b"aaa"))]);
    assert_eq!(slow.call_count(), 1);
    assert_eq!(quick.call_count(), 0);
    let stats = stats(&waiting);
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.compilations, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cache_waits_for_another_that_is_compiling_the_same_layer() {
    let dir = TempDir::new().unwrap();
    let started = Arc::new(Notify::new());
    let slow = FakeCompiler::slow("v1", started.clone(), HOLD);
    let slow_calls = slow.clone();
    let path = dir.path().to_path_buf();

    let building = tokio::spawn(async move {
        let compiler = CachedCompiler::new(slow, Some(open_at(path, HOUR, BUDGET)));
        compiler.compile(&[layer(b"aaa")]).await.unwrap()
    });
    started.notified().await;

    let inner = FakeCompiler::new("v1");
    let waiting = compiler(&dir, inner.clone(), BUDGET);
    let served = waiting.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(building.await.unwrap(), vec![Some(compiled(b"aaa"))]);
    assert_eq!(served, vec![Some(compiled(b"aaa"))]);
    assert_eq!(slow_calls.call_count(), 1);
    assert_eq!(inner.call_count(), 0);
    let stats = stats(&waiting);
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.compilations, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_what_is_still_missing_after_the_wait_is_compiled() {
    let dir = TempDir::new().unwrap();
    let started = Arc::new(Notify::new());
    let slow = FakeCompiler::slow("v1", started.clone(), HOLD);
    let path = dir.path().to_path_buf();

    let building = tokio::spawn(async move {
        let compiler = CachedCompiler::new(slow, Some(open_at(path, HOUR, BUDGET)));
        compiler.compile(&[layer(b"aaa")]).await.unwrap()
    });
    started.notified().await;

    let inner = FakeCompiler::new("v1");
    let waiting = compiler(&dir, inner.clone(), BUDGET);
    let answered = waiting
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    assert_eq!(building.await.unwrap(), vec![Some(compiled(b"aaa"))]);
    assert_eq!(
        answered,
        vec![Some(compiled(b"aaa")), Some(compiled(b"bbb"))]
    );
    assert_eq!(inner.calls(), vec![vec![b"bbb".to_vec()]]);
    let stats = stats(&waiting);
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.compilations, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_entry_that_expires_before_the_wait_is_over_counts_an_expiration() {
    let dir = TempDir::new().unwrap();
    let started = Arc::new(Notify::new());
    let slow = FakeCompiler::slow("v1", started.clone(), HOLD);
    let path = dir.path().to_path_buf();

    let building = tokio::spawn(async move {
        let compiler = CachedCompiler::new(slow, Some(open_at(path, Duration::ZERO, BUDGET)));
        compiler.compile(&[layer(b"aaa")]).await.unwrap()
    });
    started.notified().await;

    let inner = FakeCompiler::new("v1");
    let waiting = CachedCompiler::new(
        inner.clone(),
        Some(open_at(dir.path().to_path_buf(), Duration::ZERO, BUDGET)),
    );
    let answered = waiting.compile(&[layer(b"aaa")]).await.unwrap();

    building.await.unwrap();
    assert_eq!(answered, vec![Some(compiled(b"aaa"))]);
    assert_eq!(inner.call_count(), 1, "the layer had to be compiled again");
    let stats = stats(&waiting);
    assert_eq!(stats.misses, 1, "the wait does not count a second miss");
    assert_eq!(
        stats.expirations, 1,
        "the entry found after the wait had outlived its ttl"
    );
    assert_eq!(stats.hits, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cache_does_not_wait_for_a_layer_it_already_holds() {
    let dir = TempDir::new().unwrap();
    let max_bytes = 2 * LAYER_SIZE;
    let filling = compiler(&dir, FakeCompiler::new("v1"), max_bytes);
    filling.compile(&[layer(b"aaa")]).await.unwrap();
    filling.compile(&[layer(b"bbb")]).await.unwrap();
    drop(filling);

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let held = FakeCompiler::gated("v1", started.clone(), release.clone());
    let path = dir.path().to_path_buf();
    let building = tokio::spawn(async move {
        let compiler = CachedCompiler::new(held, Some(open_at(path, HOUR, max_bytes)));
        compiler.compile(&[layer(b"ccc")]).await.unwrap()
    });
    tokio::time::timeout(PATIENCE, started.notified())
        .await
        .expect("the unrelated compilation should start");

    let inner = FakeCompiler::new("v1");
    let reader_inner = inner.clone();
    let path = dir.path().to_path_buf();
    let reading = tokio::spawn(async move {
        let compiler = CachedCompiler::new(reader_inner, Some(open_at(path, HOUR, max_bytes)));
        compiler.compile(&[layer(b"aaa")]).await.unwrap()
    });
    let served = tokio::time::timeout(PATIENCE, reading).await;

    // Release the compiler even if the all-hit request timed out or panicked.
    release.notify_one();
    let built = tokio::time::timeout(PATIENCE, building)
        .await
        .expect("the released compilation should finish")
        .unwrap();
    let served = served
        .expect("opening a cache and serving a hit must finish before the compilation is released")
        .unwrap();
    assert_eq!(served, vec![Some(compiled(b"aaa"))]);
    assert_eq!(inner.call_count(), 0);
    assert_eq!(built, vec![Some(compiled(b"ccc"))]);

    // The concurrent hit must refresh the shared order before ccc evicts a layer.
    let inner = FakeCompiler::new("v1");
    let checking = compiler(&dir, inner.clone(), max_bytes);
    assert_eq!(
        checking.compile(&[layer(b"aaa")]).await.unwrap(),
        vec![Some(compiled(b"aaa"))]
    );
    assert_eq!(
        inner.call_count(),
        0,
        "the hit during compilation must protect aaa from eviction"
    );
    assert_eq!(
        checking.compile(&[layer(b"bbb")]).await.unwrap(),
        vec![Some(compiled(b"bbb"))]
    );
    assert_eq!(
        inner.calls(),
        vec![vec![b"bbb".to_vec()]],
        "bbb must be evicted after the concurrent hit refreshes aaa"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cache_waits_even_when_it_wants_a_different_layer() {
    let dir = TempDir::new().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(Notify::new());
    let slow = FakeCompiler::slow("v1", started.clone(), HOLD).logging(log.clone(), "first");
    let path = dir.path().to_path_buf();

    let building = tokio::spawn(async move {
        let compiler = CachedCompiler::new(slow, Some(open_at(path, HOUR, BUDGET)));
        compiler.compile(&[layer(b"aaa")]).await.unwrap()
    });
    started.notified().await;

    let waiting = compiler(
        &dir,
        FakeCompiler::new("v1").logging(log.clone(), "second"),
        BUDGET,
    );
    waiting.compile(&[layer(b"bbb")]).await.unwrap();
    building.await.unwrap();

    assert_eq!(
        log.lock().unwrap().clone(),
        ["first start", "first end", "second start", "second end"]
    );
}

#[test]
fn a_compilation_in_another_process_holds_the_directory() {
    let dir = TempDir::new().unwrap();
    let marks = TempDir::new().unwrap();
    let releasing = marks.path().join("releasing");

    let mut helper = TestProcess::spawn(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", HELPER, "--test-threads=1", "--nocapture"])
            .env(HELPER_CACHE, dir.path())
            .env(HELPER_MARKS, marks.path()),
    );
    helper.wait_for(&marks.path().join("holding"));

    std::fs::write(marks.path().join("ready"), b"").unwrap();

    let inner = FakeCompiler::new("v1").watching(releasing);
    let compiler = CachedCompiler::new(
        inner.clone(),
        Some(open_at(dir.path().to_path_buf(), HOUR, BUDGET)),
    );
    let answered = block_on(compiler.compile(&[layer(b"bbb")])).unwrap();

    helper.finish();
    assert_eq!(answered, vec![Some(compiled(b"bbb"))]);
    assert_eq!(
        inner.observed(),
        vec![true],
        "compiled while another process was still holding the directory"
    );
    assert_eq!(stats(&compiler).entries, 2);
}

/// The other half of `a_compilation_in_another_process_holds_the_directory`,
/// which runs it in a second process. Elsewhere it has no directory to hold and
/// returns at once.
#[test]
fn the_directory_is_held_for_another_process() {
    let (Ok(dir), Ok(marks)) = (std::env::var(HELPER_CACHE), std::env::var(HELPER_MARKS)) else {
        return;
    };
    let compiler = CachedCompiler::new(
        FakeCompiler::marking(marks.into()),
        Some(open_at(dir.into(), HOUR, BUDGET)),
    );

    let answered = block_on(compiler.compile(&[layer(b"aaa")])).unwrap();

    assert_eq!(answered, vec![Some(compiled(b"aaa"))]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caches_working_at_once_do_not_lose_entries() {
    let dir = TempDir::new().unwrap();
    let mut tasks = Vec::new();

    for task in 0..8u32 {
        let path = dir.path().to_path_buf();
        tasks.push(tokio::spawn(async move {
            let compiler =
                CachedCompiler::new(FakeCompiler::new("v1"), Some(open_at(path, HOUR, BUDGET)));
            for index in 0..8u32 {
                compiler
                    .compile(&[layer(&payload(task, index))])
                    .await
                    .unwrap();
            }
            for index in 0..8u32 {
                compiler
                    .compile(&[layer(&payload(task, index))])
                    .await
                    .unwrap();
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    let stats = open(&dir, HOUR, BUDGET).stats();
    assert_eq!(stats.entries, 64);
    assert_eq!(stats.bytes, 64 * WIDE_SIZE);
    assert_eq!(stats.swept, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caches_working_at_once_keep_the_size_bound() {
    let dir = TempDir::new().unwrap();
    let bound = 10 * WIDE_SIZE;
    let mut tasks = Vec::new();

    for task in 0..8u32 {
        let path = dir.path().to_path_buf();
        tasks.push(tokio::spawn(async move {
            let compiler =
                CachedCompiler::new(FakeCompiler::new("v1"), Some(open_at(path, HOUR, bound)));
            for index in 0..8u32 {
                compiler
                    .compile(&[layer(&payload(task, index))])
                    .await
                    .unwrap();
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    let stats = open(&dir, HOUR, bound).stats();
    assert!(stats.bytes <= bound, "{} bytes stored", stats.bytes);
    assert_eq!(stats.bytes, stats.entries * WIDE_SIZE);
    assert_eq!(stats.swept, 0);
    assert!(stats.entries > 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_layer_is_keyed_by_what_it_holds_and_not_by_how_it_is_described() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    let again = compiler
        .compile(&[differently_described(b"aaa")])
        .await
        .unwrap();

    assert_eq!(again, vec![Some(compiled(b"aaa"))]);
    assert_eq!(
        inner.call_count(),
        1,
        "the contents had already been compiled"
    );
    let stats = stats(&compiler);
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.entries, 1, "one set of contents, one entry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_layer_served_before_the_wait_is_not_counted_again_after_it() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling.compile(&[layer(b"bbb")]).await.unwrap();

    let started = Arc::new(Notify::new());
    let slow = FakeCompiler::slow("v1", started.clone(), HOLD);
    let path = dir.path().to_path_buf();
    let building = tokio::spawn(async move {
        let compiler = CachedCompiler::new(slow, Some(open_at(path, HOUR, BUDGET)));
        compiler.compile(&[layer(b"aaa")]).await.unwrap()
    });
    started.notified().await;

    let inner = FakeCompiler::new("v1");
    let waiting = compiler(&dir, inner.clone(), BUDGET);
    let answered = waiting
        .compile(&[layer(b"bbb"), layer(b"aaa")])
        .await
        .unwrap();

    assert_eq!(building.await.unwrap(), vec![Some(compiled(b"aaa"))]);
    assert_eq!(
        answered,
        vec![Some(compiled(b"bbb")), Some(compiled(b"aaa"))]
    );
    assert_eq!(inner.call_count(), 0, "both layers came from the directory");
    let stats = stats(&waiting);
    assert_eq!(
        stats.hits, 2,
        "one served on the way in, one once the wait was over"
    );
    assert_eq!(stats.misses, 1, "only the layer being built was missing");
    assert_eq!(stats.compilations, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn serving_a_layer_does_not_put_off_its_expiry() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let ttl = Duration::from_secs(4);
    let compiler = CachedCompiler::new(inner.clone(), Some(open(&dir, ttl, BUDGET)));

    let stored = std::time::Instant::now();
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    // late in the entry's life, but still inside it
    sleep_until(stored + Duration::from_secs(3));
    compiler.compile(&[layer(b"aaa")]).await.unwrap();
    assert_eq!(stats(&compiler).hits, 1, "the entry was still usable");

    // past the deadline the store set, and nowhere near one the serve would
    // have pushed out to
    sleep_until(stored + Duration::from_secs(5));
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(
        inner.call_count(),
        2,
        "the second call was served, the third not"
    );
    let stats = stats(&compiler);
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.expirations, 1, "the ttl ran from the store");
    assert_eq!(stats.misses, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn the_compiler_is_given_the_layers_in_the_order_they_first_appear() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);

    // first appearance, not the order they would sort or hash into
    let answered = compiler
        .compile(&[
            layer(b"ddd"),
            layer(b"bbb"),
            layer(b"ddd"),
            layer(b"aaa"),
            layer(b"bbb"),
        ])
        .await
        .unwrap();

    assert_eq!(
        answered,
        vec![
            Some(compiled(b"ddd")),
            Some(compiled(b"bbb")),
            Some(compiled(b"ddd")),
            Some(compiled(b"aaa")),
            Some(compiled(b"bbb")),
        ]
    );
    assert_eq!(
        inner.calls(),
        vec![vec![b"ddd".to_vec(), b"bbb".to_vec(), b"aaa".to_vec()]],
        "each distinct layer once, in the order it was first asked for"
    );
    assert_eq!(stats(&compiler).compilations, 3);
}

#[tokio::test(flavor = "current_thread")]
async fn a_cache_whose_directory_is_a_file_still_answers() {
    let dir = TempDir::new().unwrap();
    let cache_dir = dir.path().join("cache");
    let inner = FakeCompiler::new("v1");
    let compiler = CachedCompiler::new(
        inner.clone(),
        Some(open_at(cache_dir.clone(), HOUR, BUDGET)),
    );
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    // nothing about the directory can be read or written any more
    std::fs::remove_dir_all(&cache_dir).unwrap();
    std::fs::write(&cache_dir, b"where the cache used to be").unwrap();

    let answered = compiler
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    assert_eq!(
        answered,
        vec![Some(compiled(b"aaa")), Some(compiled(b"bbb"))]
    );
    assert_eq!(inner.call_count(), 2, "nothing could be served");
    assert_eq!(stats(&compiler).entries, 0);
    assert_eq!(stats(&compiler).hits, 0);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_layer_that_cannot_be_read_at_all_is_not_served() {
    let dir = TempDir::new().unwrap();
    let inner = FakeCompiler::new("v1");
    let compiler = compiler(&dir, inner.clone(), BUDGET);
    compiler.compile(&[layer(b"aaa")]).await.unwrap();

    // a link to itself: reading it fails outright rather than coming back wrong
    let stored = stored_layer(&dir, &compiled(b"aaa"));
    std::fs::remove_file(&stored).unwrap();
    std::os::unix::fs::symlink(&stored, &stored).unwrap();

    let answered = compiler.compile(&[layer(b"aaa")]).await.unwrap();

    assert_eq!(answered, vec![Some(compiled(b"aaa"))]);
    assert_eq!(inner.call_count(), 2, "it had to be compiled again");
    let stats = stats(&compiler);
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 2);
    assert_eq!(
        stats.rejected, 1,
        "a layer that will not read back is refused"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_layer_served_by_one_cache_keeps_what_another_stores_meanwhile() {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let budget = 64 * 1024 * 1024;
    let wide = vec![b'w'; 4 * 1024 * 1024];
    for round in 0..4u32 {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let filling = CachedCompiler::new(
            FakeCompiler::new("v1"),
            Some(open_at(path.clone(), HOUR, budget)),
        );
        filling.compile(&[layer(&wide)]).await.unwrap();
        let serving = CachedCompiler::new(
            FakeCompiler::new("v1"),
            Some(open_at(path.clone(), HOUR, budget)),
        );
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let storing = CachedCompiler::new(
            FakeCompiler::gated("v1", started.clone(), release.clone()),
            Some(open_at(path.clone(), HOUR, budget)),
        );
        let writer = tokio::spawn(async move {
            let kept = payload(round, 0);
            assert_eq!(
                storing.compile(&[layer(&kept)]).await.unwrap(),
                vec![Some(compiled(&kept))]
            );
        });
        tokio::time::timeout(PATIENCE, started.notified())
            .await
            .expect("the competing compiler should start");

        // The public contract stores a layer as a file holding its bytes. Observe
        // that file being opened, without naming or parsing any bookkeeping.
        let stored = std::fs::read_dir(&path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|file| std::fs::read(file).ok().as_deref() == Some(compiled(&wide).as_slice()))
            .expect("the populated layer should have a file");
        // inotify_init1 returns a new descriptor, owned by the File below.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0, "{}", std::io::Error::last_os_error());
        let mut events = unsafe { std::fs::File::from_raw_fd(fd) };
        let name = std::ffi::CString::new(stored.as_os_str().as_bytes()).unwrap();
        // Both the owned descriptor and the CString remain valid for this call.
        let watched =
            unsafe { libc::inotify_add_watch(events.as_raw_fd(), name.as_ptr(), libc::IN_OPEN) };
        assert!(watched >= 0, "{}", std::io::Error::last_os_error());
        let served = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = served.clone();
        let observer = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + PATIENCE;
            let mut buffer = [0u8; 4096];
            loop {
                match events.read(&mut buffer) {
                    Ok(count) if count > 0 => break,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    other => panic!("could not observe the layer read: {other:?}"),
                }
                if done.load(std::sync::atomic::Ordering::Acquire)
                    || std::time::Instant::now() >= deadline
                {
                    break;
                }
                sleep(Duration::from_millis(1));
            }
            // A valid implementation may serve an existing mapping without opening
            // the file again. Observation schedules the race; it is not an assertion.
            release.notify_one();
        });
        assert_eq!(
            serving.compile(&[layer(&wide)]).await.unwrap(),
            vec![Some(compiled(&wide))]
        );
        served.store(true, std::sync::atomic::Ordering::Release);
        observer.await.unwrap();
        writer.await.unwrap();
        let reopened =
            CachedCompiler::new(FakeCompiler::new("v1"), Some(open_at(path, HOUR, budget)));
        assert_eq!(
            stats(&reopened).entries,
            2,
            "serving must preserve the completed concurrent store"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_compiler_without_a_cache_leaves_precompiled_layers_to_the_wrapped_one() {
    let inner = FakeCompiler::new("v1");
    let compiler = CachedCompiler::new(inner.clone(), None);

    let answered = compiler
        .compile(&[precompiled_layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();

    assert_eq!(
        answered,
        vec![Some(compiled(b"aaa")), Some(compiled(b"bbb"))],
        "with nothing to cache the wrapped compiler's answer is what comes back"
    );
    assert_eq!(
        inner.calls(),
        vec![vec![b"aaa".to_vec(), b"bbb".to_vec()]],
        "every layer reaches the wrapped compiler"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_entry_whose_layer_became_a_directory_is_dropped_when_the_cache_is_opened() {
    let dir = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();
    let stored = stored_layer(&dir, &compiled(b"aaa"));
    std::fs::remove_file(&stored).unwrap();
    std::fs::create_dir(&stored).unwrap();

    let reopened = open(&dir, HOUR, BUDGET).stats();

    assert_eq!(
        reopened.rejected, 1,
        "a directory is not the layer it stored"
    );
    assert_eq!(reopened.entries, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repeated_layer_counts_a_hit_for_each_position_after_the_wait() {
    let dir = TempDir::new().unwrap();
    let started = Arc::new(Notify::new());
    let slow = FakeCompiler::slow("v1", started.clone(), HOLD);
    let path = dir.path().to_path_buf();
    let building = tokio::spawn(async move {
        let compiler = CachedCompiler::new(slow, Some(open_at(path, HOUR, BUDGET)));
        compiler.compile(&[layer(b"aaa")]).await.unwrap()
    });
    started.notified().await;

    let inner = FakeCompiler::new("v1");
    let waiting = compiler(&dir, inner.clone(), BUDGET);
    let answered = waiting
        .compile(&[layer(b"aaa"), layer(b"aaa")])
        .await
        .unwrap();

    building.await.unwrap();
    assert_eq!(
        answered,
        vec![Some(compiled(b"aaa")), Some(compiled(b"aaa"))]
    );
    assert_eq!(inner.call_count(), 0);
    let stats = stats(&waiting);
    assert_eq!(stats.hits, 2, "each position was looked up and served");
    assert_eq!(stats.misses, 2);
}

fn in_unprivileged_child_process(test: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    const CHILD: &str = "RUNWASI_CACHE_TEST_UNPRIVILEGED";
    // geteuid only reads the process's effective user ID.
    let uid = unsafe { libc::geteuid() };
    if std::env::var(CHILD).ok().as_deref() == Some(test) {
        assert_ne!(uid, 0, "permission-denial assertions must run without root");
        return true;
    }

    // Keep a root-built test executable reachable after the child drops its UID.
    let launch = TempDir::new_in("/tmp").unwrap();
    std::fs::set_permissions(launch.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let executable = launch.path().join("cache-test");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let work = launch.path().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o777)).unwrap();

    let mut command = Command::new(executable);
    command
        .args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env(CHILD, test)
        .env("TMPDIR", &work)
        .current_dir(&work);
    if uid == 0 {
        command.gid(65534).uid(65534);
    }
    TestProcess::spawn(&mut command).finish();
    false
}

struct RestoreDirectoryPermissions {
    path: PathBuf,
    permissions: std::fs::Permissions,
}

impl Drop for RestoreDirectoryPermissions {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, self.permissions.clone());
    }
}

#[test]
fn a_cache_recovers_after_directory_access_is_denied() {
    use std::os::unix::fs::PermissionsExt;

    if !in_unprivileged_child_process("a_cache_recovers_after_directory_access_is_denied") {
        return;
    }

    for mode in [0o555, 0o000] {
        let dir = TempDir::new().unwrap();
        let inner = FakeCompiler::new("v1");
        let cached = compiler(&dir, inner.clone(), BUDGET);
        block_on(cached.compile(&[layer(b"aaa")])).unwrap();
        let stored = stored_layer(&dir, &compiled(b"aaa"));
        let original = std::fs::metadata(dir.path()).unwrap().permissions();
        let restore = RestoreDirectoryPermissions {
            path: dir.path().to_path_buf(),
            permissions: original.clone(),
        };
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        let denied = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.path().join("permission-probe"))
            .expect_err("the test directory must actually deny new writes");
        assert_eq!(denied.kind(), std::io::ErrorKind::PermissionDenied);
        if mode == 0o555 {
            assert_eq!(std::fs::read(&stored).unwrap(), compiled(b"aaa"));
        } else {
            assert_eq!(
                std::fs::read(&stored).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }

        for bytes in [b"aaa", b"bbb", b"bbb"] {
            assert_eq!(
                block_on(cached.compile(&[layer(bytes)])).unwrap(),
                vec![Some(compiled(bytes))]
            );
        }
        assert_eq!(
            inner.calls(),
            vec![
                vec![b"aaa".to_vec()],
                vec![b"aaa".to_vec()],
                vec![b"bbb".to_vec()],
                vec![b"bbb".to_vec()],
            ],
            "unavailable persistence must cause misses, including after failed stores"
        );
        let observed = stats(&cached);
        assert_eq!(observed.hits, 0);
        assert_eq!(observed.misses, 4);
        assert_eq!(observed.compilations, 4);

        std::fs::set_permissions(dir.path(), original).unwrap();
        drop(restore);
        for _ in 0..2 {
            assert_eq!(
                block_on(cached.compile(&[layer(b"bbb")])).unwrap(),
                vec![Some(compiled(b"bbb"))]
            );
        }
        assert_eq!(
            inner.call_count(),
            5,
            "successful storage must restore hits"
        );
        let observed = stats(&cached);
        assert_eq!(observed.hits, 1);
        assert_eq!(observed.misses, 5);
        assert_eq!(observed.compilations, 5);

        let fresh_inner = FakeCompiler::new("v1");
        let fresh = compiler(&dir, fresh_inner.clone(), BUDGET);
        assert_eq!(
            block_on(fresh.compile(&[layer(b"bbb")])).unwrap(),
            vec![Some(compiled(b"bbb"))]
        );
        assert_eq!(
            fresh_inner.call_count(),
            0,
            "the recovered store must persist"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn one_store_evicts_every_entry_needed_to_fit() {
    let dir = TempDir::new().unwrap();
    let max_bytes = 4 * LAYER_SIZE;
    let large = vec![b'w'; 3 * LAYER_SIZE as usize - b"compiled:".len()];
    let filling = compiler(&dir, FakeCompiler::new("v1"), max_bytes);
    for bytes in [b"aaa", b"bbb", b"ccc", b"ddd"] {
        filling.compile(&[layer(bytes)]).await.unwrap();
    }
    filling.compile(&[layer(b"aaa")]).await.unwrap();
    assert_eq!(
        filling.compile(&[layer(&large)]).await.unwrap(),
        vec![Some(compiled(&large))]
    );
    let observed = stats(&filling);
    assert_eq!(observed.evictions, 3);
    assert_eq!(observed.entries, 2);
    assert_eq!(observed.bytes, max_bytes);
    drop(filling);

    let inner = FakeCompiler::new("v1");
    let after = compiler(&dir, inner.clone(), max_bytes);
    assert_eq!(
        after
            .compile(&[layer(b"aaa"), layer(&large)])
            .await
            .unwrap(),
        vec![Some(compiled(b"aaa")), Some(compiled(&large))]
    );
    assert_eq!(
        inner.call_count(),
        0,
        "the most recently used entry survives"
    );
    assert_eq!(
        after
            .compile(&[layer(b"bbb"), layer(b"ccc"), layer(b"ddd")])
            .await
            .unwrap(),
        vec![
            Some(compiled(b"bbb")),
            Some(compiled(b"ccc")),
            Some(compiled(b"ddd")),
        ]
    );
    assert_eq!(
        inner.calls(),
        vec![vec![b"bbb".to_vec(), b"ccc".to_vec(), b"ddd".to_vec()]]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn opening_rejects_a_readable_symlink_to_matching_layer_bytes() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let filling = compiler(&dir, FakeCompiler::new("v1"), BUDGET);
    filling
        .compile(&[layer(b"aaa"), layer(b"bbb")])
        .await
        .unwrap();
    drop(filling);
    let stored = stored_layer(&dir, &compiled(b"aaa"));
    let target = outside.path().join("matching-layer");
    std::fs::write(&target, compiled(b"aaa")).unwrap();
    std::fs::remove_file(&stored).unwrap();
    std::os::unix::fs::symlink(&target, &stored).unwrap();
    assert_eq!(std::fs::read(&stored).unwrap(), compiled(b"aaa"));

    let reopened = open(&dir, HOUR, BUDGET);
    let observed = reopened.stats();
    assert_eq!(
        observed.rejected, 1,
        "a readable symlink is not a regular layer"
    );
    assert_eq!(observed.entries, 1);
    assert_eq!(observed.bytes, LAYER_SIZE);
    let inner = FakeCompiler::new("v1");
    let after = CachedCompiler::new(inner.clone(), Some(reopened));
    assert_eq!(
        after.compile(&[layer(b"bbb")]).await.unwrap(),
        vec![Some(compiled(b"bbb"))]
    );
    assert_eq!(inner.call_count(), 0);
    assert_eq!(
        after.compile(&[layer(b"aaa")]).await.unwrap(),
        vec![Some(compiled(b"aaa"))]
    );
    assert_eq!(inner.calls(), vec![vec![b"aaa".to_vec()]]);
}
