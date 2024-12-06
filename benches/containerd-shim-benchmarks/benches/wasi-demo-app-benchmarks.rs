use std::process::Command;
use std::time::{Duration, Instant};
use criterion::{criterion_group, criterion_main, Criterion};

fn run_container(runtime: &str) -> Duration {
    let start = Instant::now();
    
    let output = Command::new("sudo")
        .args([
            "ctr",
            "run",
            "--rm",
            &format!("--runtime=io.containerd.{}.v1", runtime),
            "ghcr.io/containerd/runwasi/wasi-demo-app:latest",
            "testwasm",
            "/wasi-demo-app.wasm",
            "echo",
            "hello"
        ])
        .output()
        .expect("Failed to execute command");

    if !output.status.success() {
        panic!("Container failed to run: {}", String::from_utf8_lossy(&output.stderr));
    } else {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(stdout_str.contains("hello"));
    }

    start.elapsed()
}

fn benchmark_startup(c: &mut Criterion) {
    let mut group = c.benchmark_group("wasi-demo-app");
    
    group.bench_function("wasmtime", |b| {
        b.iter(|| run_container("wasmtime"));
    });

    group.bench_function("wasmedge", |b| {
        b.iter(|| run_container("wasmedge"));
    });

    group.bench_function("wasmer", |b| {
        b.iter(|| run_container("wasmer"));
    });

    group.bench_function("wamr", |b| {
        b.iter(|| run_container("wamr"));
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = benchmark_startup
}

criterion_main!(benches); 