use std::io::Result;

#[derive(Default, Clone)]
pub struct Stdio;

impl Stdio {
    pub fn redirect(self) -> Result<()> {
        Ok(())
    }

    pub fn take(&self) -> Self {
        Self
    }

    pub fn init_from_cfg<T>(_: T) -> Result<Self> {
        Ok(Self)
    }
}
