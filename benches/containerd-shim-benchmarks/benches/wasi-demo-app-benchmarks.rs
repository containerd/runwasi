use std::process::Command;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};

static RUNTIMES: &[&str] = &["wasmtime", "wasmedge", "wasmer", "wamr"];

fn run_container(runtime: &str, image_base_name: &str) -> Duration {
    let start = Instant::now();

    let output = Command::new("sudo")
        .args([
            "ctr",
            "run",
            "--rm",
            &format!("--runtime=io.containerd.{}.v1", runtime),
            &format!("ghcr.io/containerd/runwasi/{}:latest", image_base_name),
            "testwasm",
            &format!("{}.wasm", image_base_name),
            "echo",
            "hello",
        ])
        .output()
        .expect("Failed to execute command");

    if !output.status.success() {
        panic!(
            "Container failed to run: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    } else {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(stdout_str.contains("hello"));
    }

    start.elapsed()
}

fn benchmark_startup(c: &mut Criterion) {
    let mut group = c.benchmark_group("wasi-demo-app");

    for runtime in RUNTIMES {
        group.bench_function(*runtime, |b| {
            b.iter(|| run_container(runtime, "wasi-demo-app"));
        });
    }
    for runtime in RUNTIMES {
        let name = format!("{}-oci", runtime);
        group.bench_function(&name, |b| {
            b.iter(|| run_container(runtime, "wasi-demo-oci"));
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = benchmark_startup
}

criterion_main!(benches);
