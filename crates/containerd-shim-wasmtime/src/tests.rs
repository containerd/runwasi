use std::time::Duration;

use WasmtimeTestInstance as WasiInstance;
use containerd_shim_wasm::container::Instance;
use containerd_shim_wasm::testing::modules::*;
use containerd_shim_wasm::testing::{WasiTest, oci_helpers};
use serial_test::serial;

use crate::instance::WasmtimeEngine;

// use test configuration to avoid dead locks when running tests
// https://github.com/containerd/runwasi/issues/357
type WasmtimeTestInstance = Instance<WasmtimeEngine>;

#[tokio::test]
#[serial]
async fn test_delete_after_create() -> anyhow::Result<()> {
    WasiTest::<WasiInstance>::builder()?
        .build()
        .await?
        .delete()
        .await?;
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_hello_world() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_hello_world_oci() -> anyhow::Result<()> {
    let (builder, _oci_cleanup) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .as_oci_image(None, None)?;

    let (exit_code, stdout, _) = builder
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_hello_world_oci_uses_precompiled() -> anyhow::Result<()> {
    let (builder, _oci_cleanup1) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .as_oci_image(
            Some("localhost/hello:latest".to_string()),
            Some("c1".to_string()),
        )?;

    let (exit_code, stdout, _) = builder
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    let (label, _id) = oci_helpers::get_content_label()?;
    assert!(
        label.starts_with("runwasi.io/precompiled/wasmtime/"),
        "was {}",
        label
    );

    // run second time, it should succeed without recompiling
    let (builder, _oci_cleanup2) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .as_oci_image(
            Some("localhost/hello:latest".to_string()),
            Some("c2".to_string()),
        )?;

    let (exit_code, stdout, _) = builder
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_hello_world_oci_uses_precompiled_when_content_removed() -> anyhow::Result<()> {
    let (builder, _oci_cleanup1) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .as_oci_image(
            Some("localhost/hello:latest".to_string()),
            Some("c1".to_string()),
        )?;

    let (exit_code, stdout, _) = builder
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    // remove the compiled content from the cache
    let (label, id) = oci_helpers::get_content_label()?;
    assert!(
        label.starts_with("runwasi.io/precompiled/wasmtime/"),
        "was {}",
        label
    );
    oci_helpers::remove_content(id)?;

    // run second time, it should succeed
    let (builder, _oci_cleanup2) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WORLD)?
        .as_oci_image(
            Some("localhost/hello:latest".to_string()),
            Some("c2".to_string()),
        )?;

    let (exit_code, stdout, _) = builder
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_custom_entrypoint() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
        .with_start_fn("foo")
        .with_wasm(CUSTOM_ENTRYPOINT)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "hello world\n");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_unreachable() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(UNREACHABLE)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_ne!(exit_code, 0);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_exit_code() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(EXIT_CODE)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 42);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_seccomp() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(SECCOMP)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout.trim(), "current working dir: /");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_has_default_devices() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HAS_DEFAULT_DEVICES)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);

    Ok(())
}

// Test that the shim can execute an named exported function
// that is not the default _start function in a wasm component.
// The current limitation is that there is no way to pass arguments
// to the exported function.
// Issue that tracks this: https://github.com/containerd/runwasi/issues/414
#[tokio::test]
#[serial]
async fn test_simple_component() -> anyhow::Result<()> {
    let (exit_code, _, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(SIMPLE_COMPONENT)?
        .with_start_fn("thunk")
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);

    Ok(())
}

// Test that the shim can execute a wasm component that is
// compiled with wasip2.
//
// This is using the `wasi:cli/command` world to run the component.
//
// The wasm component is built and copied over from
// https://github.com/Mossaka/wasm-component-hello-world. See
// README.md for how to build the component.
#[tokio::test]
#[serial]
async fn test_wasip2_component() -> anyhow::Result<()> {
    let (exit_code, stdout, _) = WasiTest::<WasiInstance>::builder()?
        .with_wasm(COMPONENT_HELLO_WORLD)?
        .build()
        .await?
        .start()
        .await?
        .wait(Duration::from_secs(10))
        .await?;

    assert_eq!(exit_code, 0);
    assert_eq!(stdout, "Hello, world!\n");

    Ok(())
}

// Test that the shim can execute a wasm component that is
// compiled with wasi:http/proxy.
//
// This is using the `wasi:http/proxy` world to run the component.
//
// The wasm component is built using cargo component as illustrated in the following example::
// https://opensource.microsoft.com/blog/2024/09/25/distributing-webassembly-components-using-oci-registries/
#[tokio::test]
#[serial]
async fn test_wasip2_component_http_proxy() -> anyhow::Result<()> {
    let srv = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WASI_HTTP)?
        .with_host_network()
        .build()
        .await?;

    let srv = srv.start().await?;
    let response = http_get().await;

    let response = response.expect("Server did not start in time");
    assert!(response.status().is_success());

    let body = response.text().await.unwrap();
    assert_eq!(body, "Hello, this is your first wasi:http/proxy world!\n");

    let (exit_code, _, _) = srv.ctrl_c().await?.wait(Duration::from_secs(5)).await?;
    assert_eq!(exit_code, 0);

    Ok(())
}

// The wasm component is built using componentize-dotnet as illustrated in the following example::
// https://bytecodealliance.org/articles/simplifying-components-for-dotnet-developers-with-componentize-dotnet
// this ensures we are able to use wasm built from other languages https://github.com/containerd/runwasi/pull/723
#[tokio::test]
#[serial]
async fn test_wasip2_component_http_proxy_csharp() -> anyhow::Result<()> {
    let srv = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WASI_HTTP_CSHARP)?
        .with_host_network()
        .build()
        .await?;

    let srv = srv.start().await?;

    // dotnet takes a bit longer to start up
    // Todo: find out why this doesn't happen in wasmtime directly
    let response = http_get_with_backoff_secs(2).await;

    let response = response.expect("Server did not start in time");
    assert!(response.status().is_success());

    let body = response.text().await.unwrap();
    assert_eq!(body, "Hello, from C#!");

    let (exit_code, _, _) = srv.ctrl_c().await?.wait(Duration::from_secs(5)).await?;
    assert_eq!(exit_code, 0);

    Ok(())
}

// Test that the shim can terminate component targeting wasi:http/proxy by sending SIGTERM.
#[tokio::test]
#[serial]
async fn test_wasip2_component_http_proxy_force_shutdown() -> anyhow::Result<()> {
    let srv = WasiTest::<WasiInstance>::builder()?
        .with_wasm(HELLO_WASI_HTTP)?
        .with_host_network()
        .build()
        .await?;

    let srv = srv.start().await?;
    assert!(http_get().await.unwrap().status().is_success());

    // Send SIGTERM
    let (exit_code, _, _) = srv.terminate().await?.wait(Duration::from_secs(5)).await?;
    // The exit code indicates that the process did not exit cleanly
    assert_eq!(exit_code, 128 + libc::SIGTERM as u32);

    Ok(())
}

async fn http_get() -> reqwest::Result<reqwest::Response> {
    http_get_with_backoff_secs(1).await
}

// Helper method to make a `GET` request
async fn http_get_with_backoff_secs(backoff: u64) -> reqwest::Result<reqwest::Response> {
    const MAX_ATTEMPTS: u32 = 10;
    let backoff_duration: Duration = Duration::from_secs(backoff);

    let mut attempts = 0;

    loop {
        match reqwest::get("http://127.0.0.1:8080").await {
            Ok(resp) => break Ok(resp),
            Err(err) if attempts == MAX_ATTEMPTS => break Err(err),
            Err(_) => {
                std::thread::sleep(backoff_duration);
                attempts += 1;
            }
        }
    }
}
