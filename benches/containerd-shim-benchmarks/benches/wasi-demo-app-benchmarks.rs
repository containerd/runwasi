use std::process::Command;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};

struct TestCase<'a> {
    image: &'a str,
    entrypoint: &'a str,
    args: &'a [&'a str],
    expected: &'a str,
}

static RUNTIMES: &[&str] = &["wasmtime", "wasmedge", "wasmer", "wamr"];
static TEST_CASES: &[TestCase] = &[
    TestCase {
        image: "ghcr.io/containerd/runwasi/wasi-demo-app:latest",
        entrypoint: "wasi-demo-app.wasm",
        args: &["echo", "hello"],
        expected: "hello",
    },
    TestCase {
        image: "ghcr.io/containerd/runwasi/wasi-demo-oci:latest",
        entrypoint: "wasi-demo-oci.wasm",
        args: &["echo", "hello"],
        expected: "hello",
    },
];

fn run_container<F>(runtime: &str, test_case: &TestCase, verify_output: F) -> Duration
where
    F: Fn(&str),
{
    let start = Instant::now();
    let mut cmd = Command::new("sudo");
    cmd.args([
        "ctr",
        "run",
        "--rm",
        &format!("--runtime=io.containerd.{}.v1", runtime),
        test_case.image,
        "testwasm",
        test_case.entrypoint,
    ]);
    cmd.args(test_case.args);

    let output = cmd.output().expect("Failed to execute command");
    if !output.status.success() {
        panic!(
            "Container failed to run: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    verify_output(&stdout);

    start.elapsed()
}

fn benchmark_image(c: &mut Criterion) {
    let mut group = c.benchmark_group("end-to-end");
    for runtime in RUNTIMES {
        for test_case in TEST_CASES {
            let image_base_name = test_case
                .image
                .rsplit('/')
                .next()
                .unwrap_or(test_case.image);
            let bench_name = format!("{}/{}", runtime, image_base_name);
            group.bench_function(&bench_name, |b| {
                b.iter(|| {
                    run_container(runtime, test_case, |stdout| {
                        assert!(stdout.contains(test_case.expected));
                    })
                });
            });
        }
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(3));
    targets = benchmark_image
}

criterion_main!(benches);
