use std::borrow::Cow;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serial_test::serial;
use oci_spec::runtime::{ProcessBuilder, RootBuilder, SpecBuilder};
use wasmedge_sdk::Vm as WasmEdgeVm;
use wasmtime::Engine as WasmtimeVm;

use containerd_shim_wasm::function;
use containerd_shim_wasm::sandbox::testutil::{has_cap_sys_admin, run_test_with_sudo};
use containerd_shim_wasm::sandbox::Error;

use containerd_shim_wasmedge::instance::Wasi as WasmEdgeWasi;
use containerd_shim_wasmtime::instance::Wasi as WasmtimeWasi;

#[derive(Serialize, Deserialize)]
struct Options {
    root: Option<PathBuf>,
}

mod common;

#[test]
#[serial]
fn test_has_default_devices() -> Result<(), Error> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo("test_has_default_devices");
    }

    let wasmbytes = common::get_external_wasm_module("has-default-devices.wasm".to_string())?;

    let spec = SpecBuilder::default()
        .root(RootBuilder::default().path("rootfs").build()?)
        .process(
            ProcessBuilder::default()
                .cwd("/")
                .args(vec![common::WASM_FILENAME.to_string()])
                .build()?,
        )
        .build()?;

    let bytes = Cow::from(wasmbytes);

    let (output, retval) = common::run_test_with_spec::<WasmtimeWasi, WasmtimeVm>(&spec, &bytes)?;
    assert_eq!(retval, 0, "error: {}", output);

    let (output, retval) = common::run_test_with_spec::<WasmEdgeWasi, WasmEdgeVm>(&spec, &bytes)?;
    assert_eq!(retval, 0, "error: {}", output);

    Ok(())
}
