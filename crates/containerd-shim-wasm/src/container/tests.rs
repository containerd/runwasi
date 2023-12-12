use anyhow::bail;

use super::engine::RuntimeInfo;
use crate::container::{Engine, RuntimeContext, Stdio};
use crate::sys::container::instance::Instance;
use crate::testing::WasiTest;

#[derive(Clone, Default)]
struct EngineFailingValidation;

impl Engine for EngineFailingValidation {
    fn info() -> &'static RuntimeInfo {
        &RuntimeInfo {
            name: "wasi_instance",
            version: "0.0.0",
        }
    }
    fn can_handle(&self, _ctx: &impl RuntimeContext) -> anyhow::Result<()> {
        bail!("can't handle");
    }
    fn run_wasi(&self, _ctx: &impl RuntimeContext, _stdio: Stdio) -> anyhow::Result<i32> {
        Ok(0)
    }
}

type InstanceFailingValidation = Instance<EngineFailingValidation>;

#[test]
#[cfg(unix)] // not yet implemented on Windows
fn test_validation_error() -> anyhow::Result<()> {
    // A validation error should fail when creating the container
    // as opposed to failing when starting it.

    let result = WasiTest::<InstanceFailingValidation>::builder()?
        .with_start_fn("foo")?
        .with_wasm("/invalid_entrypoint.wasm")?
        .build();

    assert!(result.is_err());

    Ok(())
}
