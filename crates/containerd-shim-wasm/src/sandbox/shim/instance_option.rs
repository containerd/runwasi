use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::sandbox::instance::Nop;
use crate::sandbox::{Instance, InstanceConfig, Result};

pub(super) enum InstanceOption<I: Instance> {
    Instance(I),
    Nop(Nop),
}

impl<I: Instance> Instance for InstanceOption<I> {
    type Engine = ();

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn new(_id: String, _cfg: Option<&InstanceConfig<Self::Engine>>) -> Result<Self> {
        // this is never called
        unimplemented!();
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn start(&self) -> Result<u32> {
        match self {
            Self::Instance(i) => i.start(),
            Self::Nop(i) => i.start(),
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn kill(&self, signal: u32) -> Result<()> {
        match self {
            Self::Instance(i) => i.kill(signal),
            Self::Nop(i) => i.kill(signal),
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn delete(&self) -> Result<()> {
        match self {
            Self::Instance(i) => i.delete(),
            Self::Nop(i) => i.delete(),
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip(self, t), level = "Info"))]
    fn wait_timeout(&self, t: impl Into<Option<Duration>>) -> Option<(u32, DateTime<Utc>)> {
        match self {
            Self::Instance(i) => i.wait_timeout(t),
            Self::Nop(i) => i.wait_timeout(t),
        }
    }
}
