use std::borrow::Cow;
use std::time::Duration;

use containerd_shim_wasm::container::Instance;
use containerd_shim_wasm::sandbox::Error;
use containerd_shim_wasm::testing::WasiTest;
use containerd_shim_wasmedge::instance::WasmEdgeEngine;
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
    aead_chacha20poly1305,
    aead_xchacha20poly1305,
    auth,
    auth2,
    auth3,
    auth6,
    box_seed,
    generichash2,
    generichash3,
    hash,
    hash3,
    kdf,
    keygen,
    onetimeauth,
    onetimeauth2,
    scalarmult2,
    secretbox,
    secretbox2,
    secretbox_easy,
    secretstream_xchacha20poly1305,
    shorthash,
    siphashx24,
    stream3,
    stream4

    Criterion is set to run each benchmark ten times, ten being the minimum
    number of iterations that Criterion accepts. If we need more statistically
    meaningful results we can increase the number of iterations (with the cost
    of a longer benchmarking time). Running the whole suite on a desktop
    computer takes now a bit over 10 minutes.
*/

type WasmedgeTestInstance = Instance<WasmEdgeEngine>;

fn run_wasmedge_test_with_spec(wasmbytes: &[u8]) -> Result<u32, Error> {
    let (exit_code, _, _) = WasiTest::<WasmedgeTestInstance>::builder()?
        .with_wasm(wasmbytes)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;
    Ok(exit_code)
}

fn run_wasmedge_benchmark(group: &mut BenchmarkGroup<WallTime>, bytes: &[u8]) {
    group.bench_function("Wasmedge", |b| {
        b.iter(|| {
            let res = run_wasmedge_test_with_spec(bytes);
            match res {
                Err(e) => {
                    panic!("Error running Wasmedge benchmark: {}", e);
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
            run_wasmedge_benchmark(&mut group, &bytes);
            group.finish();
        }
    };
    ($name:ident, $($rest:tt),+) => {
        bench_wasm!($name);
        bench_wasm!($($rest),+);
    };
}

bench_wasm! {
    aead_chacha20poly1305,
    aead_xchacha20poly1305,
    auth,
    auth2,
    auth3,
    auth6,
    box_seed,
    generichash2,
    generichash3,
    hash,
    hash3,
    kdf,
    keygen,
    onetimeauth,
    onetimeauth2,
    scalarmult2,
    secretbox,
    secretbox2,
    secretbox_easy,
    secretstream_xchacha20poly1305,
    shorthash,
    siphashx24,
    stream3,
    stream4
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = aead_chacha20poly1305,
        aead_xchacha20poly1305,
        auth,
        auth2,
        auth3,
        auth6,
        box_seed,
        generichash2,
        generichash3,
        hash,
        hash3,
        kdf,
        keygen,
        onetimeauth,
        onetimeauth2,
        scalarmult2,
        secretbox,
        secretbox2,
        secretbox_easy,
        secretstream_xchacha20poly1305,
        shorthash,
        siphashx24,
        stream3,
        stream4
}

criterion_main!(benches);
