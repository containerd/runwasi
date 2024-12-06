use std::process::Command;
use std::time::{Duration, Instant};
use criterion::{criterion_group, criterion_main, Criterion};

fn run_container(runtime: &str, oci: bool) -> Duration {
    let start = Instant::now();
    
    let image_name = if oci {
        "ghcr.io/containerd/runwasi/wasi-demo-oci:latest" // OCI artifact
    } else {
        "ghcr.io/containerd/runwasi/wasi-demo-app:latest"
    };

    let container_name = "testwasm";
    let wasm_file = if oci { "wasi-demo-oci.wasm" } else { "wasi-demo-app.wasm" };
    
    let output = Command::new("sudo")
        .args([
            "ctr",
            "run",
            "--rm",
            &format!("--runtime=io.containerd.{}.v1", runtime),
            image_name,
            container_name,
            wasm_file,
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
    
    const RUNTIMES: &[&str] = &["wasmtime", "wasmedge", "wasmer", "wamr"];
    
    for runtime in RUNTIMES {
        group.bench_function(runtime, |b| {
            b.iter(|| run_container(runtime, false));
        });
    }
    for runtime in RUNTIMES {
        let name = format!("{}-oci", runtime);
        group.bench_function(&name, |b| {
            b.iter(|| run_container(runtime, true));
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