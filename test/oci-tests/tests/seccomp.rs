use std::borrow::Cow;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serial_test::serial;
use oci_spec::runtime::{
    LinuxBuilder, LinuxSeccompAction, LinuxSeccompBuilder, LinuxSyscallBuilder, ProcessBuilder,
    RootBuilder, SpecBuilder,
};
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
fn test_external_hello_world() -> Result<(), Error> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo("test_external_hello_world");
    }

    let wasmbytes = common::get_external_wasm_module("hello-world.wasm".to_string())?;

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

    let (output, retval) = common::run_test_with_spec::<WasmEdgeWasi, WasmEdgeVm>(&spec, &bytes)?;
    assert_eq!(retval, 0);
    assert!(output.starts_with("hello world"));

    let (output, retval) = common::run_test_with_spec::<WasmtimeWasi, WasmtimeVm>(&spec, &bytes)?;
    assert_eq!(retval, 0);
    assert!(output.starts_with("hello world"));

    Ok(())
}

#[test]
#[serial]
fn test_seccomp_hello_world_pass() -> Result<(), Error> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo("test_seccomp_hello_world_pass");
    }

    let wasmbytes = common::get_external_wasm_module("hello-world.wasm".to_string())?;

    let spec = SpecBuilder::default()
        .root(RootBuilder::default().path("rootfs").build()?)
        .process(
            ProcessBuilder::default()
                .cwd("/")
                .args(vec![common::WASM_FILENAME.to_string()])
                .build()?,
        )
        .linux(
            LinuxBuilder::default()
                .seccomp(
                    LinuxSeccompBuilder::default()
                        .default_action(LinuxSeccompAction::ScmpActAllow)
                        .architectures(vec![oci_spec::runtime::Arch::ScmpArchNative])
                        .syscalls(vec![LinuxSyscallBuilder::default()
                            .names(vec!["getcwd".to_string()])
                            .action(LinuxSeccompAction::ScmpActAllow)
                            .build()?])
                        .build()?,
                )
                .build()?,
        )
        .build()?;

    let bytes = Cow::from(wasmbytes);

    let (output, retval) = common::run_test_with_spec::<WasmEdgeWasi, WasmEdgeVm>(&spec, &bytes)?;
    assert_eq!(retval, 0);
    assert!(output.starts_with("hello world"));

    let (output, retval) = common::run_test_with_spec::<WasmtimeWasi, WasmtimeVm>(&spec, &bytes)?;
    assert_eq!(retval, 0);
    assert!(output.starts_with("hello world"));

    Ok(())
}

#[test]
#[serial]
fn test_seccomp_hello_world_fail() -> Result<(), Error> {
    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo("test_seccomp_hello_world_fail");
    }

    let wasmbytes = common::get_external_wasm_module("hello-world.wasm".to_string())?;

    let spec = SpecBuilder::default()
        .root(RootBuilder::default().path("rootfs").build()?)
        .process(
            ProcessBuilder::default()
                .cwd("/")
                .args(vec![common::WASM_FILENAME.to_string()])
                .build()?,
        )
        .linux(
            LinuxBuilder::default()
                .seccomp(
                    LinuxSeccompBuilder::default()
                        .default_action(LinuxSeccompAction::ScmpActAllow)
                        .architectures(vec![oci_spec::runtime::Arch::ScmpArchNative])
                        .syscalls(vec![LinuxSyscallBuilder::default()
                            .names(vec!["sched_getaffinity".to_string(), "getcwd".to_string()]) // Do not allow sched_getaffinity()
                            .action(LinuxSeccompAction::ScmpActKill)
                            .build()?])
                        .build()?,
                )
                .build()?,
        )
        .build()?;

    let bytes = Cow::from(wasmbytes);

    let (_, retval) = common::run_test_with_spec::<WasmEdgeWasi, WasmEdgeVm>(&spec, &bytes)?;
    assert_ne!(retval, 0);

    let (_, retval) = common::run_test_with_spec::<WasmtimeWasi, WasmtimeVm>(&spec, &bytes)?;
    assert_ne!(retval, 0);

    Ok(())
}

#[test]
#[serial]
#[ignore]
fn test_seccomp_hello_world_notify() -> Result<(), Error> {
    // Test how seccomp works together with an external notification agent.
    // Configure the external agent to use socket /tmp/seccomp-agent.socket
    // and set it to either allow or decline (with error) "getcwd" system
    // call. Then configure success_expected to true if allowed and false
    // if declined.

    let success_expected = true;

    if !has_cap_sys_admin() {
        println!("running test with sudo: {}", function!());
        return run_test_with_sudo(function!());
    }

    let wasmbytes = common::get_external_wasm_module("hello-world.wasm".to_string())?;

    let spec = SpecBuilder::default()
        .root(RootBuilder::default().path("rootfs").build()?)
        .process(
            ProcessBuilder::default()
                .cwd("/")
                .args(vec![common::WASM_FILENAME.to_string()])
                .build()?,
        )
        .linux(
            LinuxBuilder::default()
                .seccomp(
                    LinuxSeccompBuilder::default()
                        .default_action(LinuxSeccompAction::ScmpActAllow)
                        .architectures(vec![oci_spec::runtime::Arch::ScmpArchNative])
                        .syscalls(vec![LinuxSyscallBuilder::default()
                            .names(vec!["getcwd".to_string()]) // getcwd() is checked from an external process
                            .action(LinuxSeccompAction::ScmpActNotify)
                            .build()?])
                        .listener_path("/tmp/seccomp-agent.socket")
                        .build()?,
                )
                .build()?,
        )
        .build()?;

    let bytes = Cow::from(wasmbytes);

    if success_expected {
        let (output, retval) =
            common::run_test_with_spec::<WasmEdgeWasi, WasmEdgeVm>(&spec, &bytes)?;
        assert_eq!(retval, 0);
        assert!(output.starts_with("hello world"));

        let (output, retval) =
            common::run_test_with_spec::<WasmtimeWasi, WasmtimeVm>(&spec, &bytes)?;
        assert_eq!(retval, 0);
        assert!(output.starts_with("hello world"));
    } else {
        let (_, retval) = common::run_test_with_spec::<WasmEdgeWasi, WasmEdgeVm>(&spec, &bytes)?;
        assert_ne!(retval, 0);

        let (_, retval) = common::run_test_with_spec::<WasmtimeWasi, WasmtimeVm>(&spec, &bytes)?;
        assert_ne!(retval, 0);
    }

    Ok(())
}
