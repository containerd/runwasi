use anyhow::Result;

use crate::container::RuntimeContext;
use crate::sandbox::Stdio;

pub trait Engine: Clone + Send + Sync + 'static {
    /// The name to use for this engine
    fn name() -> &'static str;

    /// Run a WebAssembly container
    fn run(&self, ctx: impl RuntimeContext, stdio: Stdio) -> Result<i32>;

    /// Check that the runtime can run the container.
    /// This checks runs after the container creation and before the container starts.
    /// By default the check always succeeeds.
    fn can_handle(&self, _ctx: impl RuntimeContext) -> Result<()> {
        Ok(())
    }
}
