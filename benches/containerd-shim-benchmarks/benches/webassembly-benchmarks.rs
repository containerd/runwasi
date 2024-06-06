use std::borrow::Cow;
use std::path::Path;
use std::time::Duration;

use anyhow::bail;
use containerd_shim_wasm::container::Instance;
use containerd_shim_wasm::sandbox::Error;
use containerd_shim_wasm::testing::WasiTest;
use containerd_shim_wasmedge::WasmEdgeInstance;
use containerd_shim_wasmtime::instance::{DefaultConfig, WasmtimeEngine};
use criterion::{criterion_group, criterion_main, Criterion};

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
type WasmtimeTestInstance = Instance<WasmtimeEngine<DefaultConfig>>;
type WasmedgeTestInstance = WasmEdgeInstance;

fn get_external_benchmark_module(name: String) -> Result<Vec<u8>, Error> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let target = Path::new(manifest_dir)
        .join("../../benches/webassembly-benchmarks/2022-12/wasm")
        .join(name.clone());
    std::fs::read(target).map_err(|e| {
        Error::Others(format!(
            "failed to read requested Wasm module ({}): {}.",
            name, e
        ))
    })
}

fn run_wasmtime_test_with_spec(wasmbytes: &[u8]) -> Result<u32, Error> {
    let (exit_code, _, _) = WasiTest::<WasmtimeTestInstance>::builder()?
        .with_wasm(wasmbytes)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;
    Ok(exit_code)
}

fn run_wasmedge_test_with_spec(wasmbytes: &[u8]) -> Result<u32, Error> {
    let (exit_code, _, _) = WasiTest::<WasmedgeTestInstance>::builder()?
        .with_wasm(wasmbytes)?
        .build()?
        .start()?
        .wait(Duration::from_secs(10))?;
    Ok(exit_code)
}

fn run_bench(c: &mut Criterion, name: &str) -> Result<(), Error> {
    let wasmbytes = get_external_benchmark_module(format!("{}.wasm", name))?;

    let bytes = Cow::from(wasmbytes);

    let mut group = c.benchmark_group(name);
    group.bench_function("Wasmtime", |b| {
        b.iter(|| {
            let res = run_wasmtime_test_with_spec(&bytes);
            match res {
                Err(e) => bail!("Error running Wasmtime benchmark: {}", e),
                Ok(status) => {
                    assert_eq!(status, 0);
                    Ok(())
                }
            }
        })
    });
    group.bench_function("WasmEdge", |b| {
        b.iter(|| {
            let res = run_wasmedge_test_with_spec(&bytes);
            match res {
                Err(e) => bail!("Error running WasmEdge benchmark: {}", e),
                Ok(status) => {
                    assert_eq!(status, 0);
                    Ok(())
                }
            }
        })
    });
    group.finish();

    Ok(())
}

fn bench_aead_chacha20poly1305(b: &mut Criterion) {
    run_bench(b, "aead_chacha20poly1305").unwrap()
}

fn bench_aead_chacha20poly13052(b: &mut Criterion) {
    run_bench(b, "aead_chacha20poly13052").unwrap()
}

fn bench_aead_xchacha20poly1305(b: &mut Criterion) {
    run_bench(b, "aead_xchacha20poly1305").unwrap()
}

fn bench_auth2(b: &mut Criterion) {
    run_bench(b, "auth2").unwrap()
}

fn bench_auth3(b: &mut Criterion) {
    run_bench(b, "auth3").unwrap()
}

fn bench_auth6(b: &mut Criterion) {
    run_bench(b, "auth6").unwrap()
}

fn bench_auth(b: &mut Criterion) {
    run_bench(b, "auth").unwrap()
}

fn bench_box_seed(b: &mut Criterion) {
    run_bench(b, "box_seed").unwrap()
}

fn bench_generichash2(b: &mut Criterion) {
    run_bench(b, "generichash2").unwrap()
}

fn bench_generichash3(b: &mut Criterion) {
    run_bench(b, "generichash3").unwrap()
}

fn bench_hash3(b: &mut Criterion) {
    run_bench(b, "hash3").unwrap()
}

fn bench_hash(b: &mut Criterion) {
    run_bench(b, "hash").unwrap()
}

fn bench_kdf(b: &mut Criterion) {
    run_bench(b, "kdf").unwrap()
}

fn bench_keygen(b: &mut Criterion) {
    run_bench(b, "keygen").unwrap()
}

fn bench_onetimeauth2(b: &mut Criterion) {
    run_bench(b, "onetimeauth2").unwrap()
}

fn bench_onetimeauth(b: &mut Criterion) {
    run_bench(b, "onetimeauth").unwrap()
}

fn bench_scalarmult2(b: &mut Criterion) {
    run_bench(b, "scalarmult2").unwrap()
}

fn bench_scalarmult5(b: &mut Criterion) {
    run_bench(b, "scalarmult5").unwrap()
}

fn bench_scalarmult6(b: &mut Criterion) {
    run_bench(b, "scalarmult6").unwrap()
}

fn bench_secretbox2(b: &mut Criterion) {
    run_bench(b, "secretbox2").unwrap()
}

fn bench_secretbox_easy(b: &mut Criterion) {
    run_bench(b, "secretbox_easy").unwrap()
}

fn bench_secretbox(b: &mut Criterion) {
    run_bench(b, "secretbox").unwrap()
}

fn bench_secretstream_xchacha20poly1305(b: &mut Criterion) {
    run_bench(b, "secretstream_xchacha20poly1305").unwrap()
}

fn bench_shorthash(b: &mut Criterion) {
    run_bench(b, "shorthash").unwrap()
}

fn bench_siphashx24(b: &mut Criterion) {
    run_bench(b, "siphashx24").unwrap()
}

fn bench_stream3(b: &mut Criterion) {
    run_bench(b, "stream3").unwrap()
}

fn bench_stream4(b: &mut Criterion) {
    run_bench(b, "stream4").unwrap()
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10).measurement_time(Duration::from_secs(100));
    targets = bench_aead_chacha20poly13052, bench_aead_chacha20poly1305, bench_aead_xchacha20poly1305, bench_auth2, bench_auth3, bench_auth6, bench_auth, bench_box_seed, bench_generichash2, bench_generichash3, bench_hash3, bench_hash, bench_kdf, bench_keygen, bench_onetimeauth2, bench_onetimeauth, bench_scalarmult2, bench_scalarmult5, bench_scalarmult6, bench_secretbox2, bench_secretbox_easy, bench_secretbox, bench_secretstream_xchacha20poly1305, bench_shorthash, bench_siphashx24, bench_stream3, bench_stream4
}

criterion_main!(benches);
