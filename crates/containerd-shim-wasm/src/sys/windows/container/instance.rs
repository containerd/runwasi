use std::marker::PhantomData;

use crate::container::Engine;
use crate::sandbox::instance::Wait;
use crate::sandbox::{Error as SandboxError, Instance as SandboxInstance, InstanceConfig};

pub struct Instance<E: Engine>(PhantomData<E>);

impl<E: Engine> SandboxInstance for Instance<E> {
    type Engine = E;

    fn new(_id: String, _cfg: Option<&InstanceConfig<Self::Engine>>) -> Self {
        todo!();
    }

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    fn start(&self) -> Result<u32, SandboxError> {
        todo!();
    }

    /// Send a signal to the instance
    fn kill(&self, _signal: u32) -> Result<(), SandboxError> {
        todo!();
    }

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    fn delete(&self) -> Result<(), SandboxError> {
        todo!();
    }

    /// Set up waiting for the instance to exit
    /// The Wait struct is used to send the exit code and time back to the
    /// caller. The recipient is expected to call function
    /// set_up_exit_code_wait() implemented by Wait to set up exit code
    /// processing. Note that the "wait" function doesn't block, but
    /// it sets up the waiting channel.
    fn wait(&self, _waiter: &Wait) -> Result<(), SandboxError> {
        todo!();
    }
}
