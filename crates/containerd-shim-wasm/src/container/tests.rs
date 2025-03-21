use anyhow::bail;

use crate::container::{Engine, RuntimeContext};
use crate::testing::WasiTest;

#[derive(Clone, Default)]
struct EngineFailingValidation;

impl Engine for EngineFailingValidation {
    fn name() -> &'static str {
        "wasi_instance"
    }
    async fn can_handle(&self, _ctx: &impl RuntimeContext) -> anyhow::Result<()> {
        bail!("can't handle");
    }
    async fn run_wasi(&self, _ctx: &impl RuntimeContext) -> anyhow::Result<i32> {
        Ok(0)
    }
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
