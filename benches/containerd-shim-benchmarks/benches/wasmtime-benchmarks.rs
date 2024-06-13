use std::borrow::Cow;
use std::time::Duration;

use containerd_shim_wasm::container::Instance;
use containerd_shim_wasm::sandbox::Error;
use containerd_shim_wasm::testing::WasiTest;
use containerd_shim_wasmtime::instance::{WasiConfig, WasmtimeEngine};
use criterion::measurement::WallTime;
use criterion::{criterion_group, criterion_main, BenchmarkGroup, Criterion};

/*
    For benchmarking try to choose cases which run fast enough -- the idea is to
    get a rough idea of the performance rather than run a hours-long benchmark
    suite.

    Because of this, only select the benchmarks which run in under 5 seconds on
    a desktop computer using WasmEdge. Note that this selection is pretty
    arbitrary and we can add or remove benchmarks easily, also from other
    sources. This is the selection algorithm:

    $ for file in *; do if timeout 5s wasmedge "${file}" > /dev/null; then echo "$file"; fi; done
    aead_chacha20poly13052.wasm
    aead_chacha20poly1305.wasm
    aead_xchacha20poly1305.wasm
    auth2.wasm
    auth3.wasm
    auth6.wasm
    auth.wasm
    box_seed.wasm
    generichash2.wasm
    generichash3.wasm
    hash3.wasm
    hash.wasm
    kdf.wasm
    keygen.wasm
    onetimeauth2.wasm
    onetimeauth.wasm
    scalarmult2.wasm
    scalarmult5.wasm
    scalarmult6.wasm
    secretbox2.wasm
    secretbox_easy.wasm
    secretbox.wasm
    secretstream_xchacha20poly1305.wasm
    shorthash.wasm
    siphashx24.wasm
    stream3.wasm
    stream4.wasm

    Criterion is set to run each benchmark ten times, ten being the minimum
    number of iterations that Criterion accepts. If we need more statistically
    meaningful results we can increase the number of iterations (with the cost
    of a longer benchmarking time). Running the whole suite on a desktop
    computer takes now a bit over 10 minutes.
*/
#[derive(Clone)]
struct WasiTestConfig {}

impl WasiConfig for WasiTestConfig {
    fn new_config() -> wasmtime::Config {
        let mut config = wasmtime::Config::new();
        // Disable Wasmtime parallel compilation for the tests
        // see https://github.com/containerd/runwasi/pull/405#issuecomment-1928468714 for details
        config.parallel_compilation(false);
        config.wasm_component_model(true); // enable component linking
        config
    }
}

type WasmtimeTestInstance = Instance<WasmtimeEngine<WasiTestConfig>>;

fn run_wasmtime_test_with_spec(wasmbytes: &[u8]) -> Result<u32, Error> {
    let (exit_code, _, _) = WasiTest::<WasmtimeTestInstance>::builder()?
        .with_wasm(wasmbytes)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;
    Ok(exit_code)
}

fn run_wasmtime_benchmark(group: &mut BenchmarkGroup<WallTime>, bytes: &[u8]) {
    group.bench_function("Wasmtime", |b| {
        b.iter(|| {
            let res = run_wasmtime_test_with_spec(bytes);
            match res {
                Err(e) => {
                    panic!("Error running Wasmtime benchmark: {}", e);
                }
                Ok(status) => {
                    assert_eq!(status, 0);
                }
            }
        })
    });
}

macro_rules! bench_wasm {
    ($name:ident) => {
        fn $name(c: &mut Criterion) {
            let wasmbytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../benches/webassembly-benchmarks/2022-12/wasm/", stringify!($name), ".wasm"));
            let bytes = Cow::from(wasmbytes);
            let mut group = c.benchmark_group(stringify!($name));
            run_wasmtime_benchmark(&mut group, &bytes);
            group.finish();
        }
    };
    ($name:ident, $($rest:tt),+) => {
        bench_wasm!($name);
        bench_wasm!($($rest),+);
    };
}

bench_wasm! {
    aead_chacha20poly13052,
    aead_chacha20poly1305,
    aead_xchacha20poly1305,
    auth2,
    auth3,
    auth6,
    auth,
    box_seed,
    generichash2,
    generichash3,
    hash3,
    hash,
    kdf,
    keygen,
    onetimeauth2,
    onetimeauth,
    scalarmult2,
    scalarmult5,
    scalarmult6,
    secretbox2,
    secretbox_easy,
    secretbox,
    secretstream_xchacha20poly1305,
    shorthash,
    siphashx24,
    stream3,
    stream4
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = aead_chacha20poly13052,
        aead_chacha20poly1305,
        aead_xchacha20poly1305,
        auth2,
        auth3,
        auth6,
        auth,
        box_seed,
        generichash2,
        generichash3,
        hash3,
        hash,
        kdf,
        keygen,
        onetimeauth2,
        onetimeauth,
        scalarmult2,
        scalarmult5,
        scalarmult6,
        secretbox2,
        secretbox_easy,
        secretbox,
        secretstream_xchacha20poly1305,
        shorthash,
        siphashx24,
        stream3,
        stream4
}

criterion_main!(benches);
