use anyhow::bail;

use super::engine::Sandbox;
use crate::container::{RuntimeContext, Shim};
use crate::testing::WasiTest;

#[derive(Clone, Default)]
struct EngineFailingValidation;

#[derive(Default)]
struct ContainerFailingValidation;

impl Sandbox for ContainerFailingValidation {
    async fn can_handle(&self, _ctx: &impl RuntimeContext) -> anyhow::Result<()> {
        bail!("can't handle");
    }
    async fn run_wasi(&self, _ctx: &impl RuntimeContext) -> anyhow::Result<i32> {
        Ok(0)
    }
}

impl Shim for EngineFailingValidation {
    fn name() -> &'static str {
        "wasi_instance"
    }

    type Sandbox = ContainerFailingValidation;
}

#[test]
#[cfg(unix)] // not yet implemented on Windows
fn test_validation_error() -> anyhow::Result<()> {
    // A validation error should fail when creating the container
    // as opposed to failing when starting it.

    let result = WasiTest::<EngineFailingValidation>::builder()?
        .with_start_fn("foo")
        .with_wasm("/invalid_entrypoint.wasm")?
        .build();

    assert!(result.is_err());

    Ok(())
}
