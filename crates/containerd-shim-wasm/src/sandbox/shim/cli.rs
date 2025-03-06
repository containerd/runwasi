use std::env::current_dir;
use std::fmt::Debug;
use std::marker::PhantomData;

use async_trait::async_trait;
use chrono::Utc;
use containerd_shim::error::Error as ShimError;
use containerd_shim::publisher::RemotePublisher;
use containerd_shim::util::write_str_to_file;
use containerd_shim::{self as shim, api};
use oci_spec::runtime::Spec;
use shim::Flags;

use crate::sandbox::async_utils::AmbientRuntime as _;
use crate::sandbox::instance::Instance;
use crate::sandbox::shim::events::{RemoteEventSender, ToTimestamp};
use crate::sandbox::shim::local::Local;
use crate::sandbox::sync::WaitableCell;

/// Cli implements the containerd-shim cli interface using `Local<T>` as the task service.
pub struct Cli<T: Instance + Sync + Send> {
    namespace: String,
    containerd_address: String,
    exit: WaitableCell<()>,
    _id: String,
    _phantom: PhantomData<T>,
}

impl<I> Debug for Cli<I>
where
    I: Instance + Sync + Send,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cli {{ namespace: {:?}, containerd_address: {:?}, _id: {:?} }}",
            self.namespace, self.containerd_address, self._id
        )
    }
}

#[async_trait]
impl<I> shim::Shim for Cli<I>
where
    I: Instance + Sync + Send,
{
    type T = Local<I>;

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Info"))]
    async fn new(_runtime_id: &str, args: &Flags, _config: &mut shim::Config) -> Self {
        Cli {
            namespace: args.namespace.to_string(),
            containerd_address: args.address.clone(),
            exit: WaitableCell::new(),
            _id: args.id.to_string(),
            _phantom: PhantomData,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Info"))]
    async fn start_shim(&mut self, opts: containerd_shim::StartOpts) -> shim::Result<String> {
        let dir = current_dir().map_err(|err| ShimError::Other(err.to_string()))?;
        let spec = Spec::load(dir.join("config.json")).map_err(|err| {
            shim::Error::InvalidArgument(format!("error loading runtime spec: {}", err))
        })?;

        let id = opts.id.clone();
        let grouping = spec
            .annotations()
            .as_ref()
            .and_then(|a| a.get("io.kubernetes.cri.sandbox-id"))
            .unwrap_or(&id);

        let address = shim::spawn(opts, grouping, vec![]).await?;

        write_str_to_file("address", &address).await?;

        Ok(address)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Info"))]
    async fn wait(&mut self) {
        self.exit.wait().block_on();
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(publisher), level = "Info")
    )]
    async fn create_task_service(&self, publisher: RemotePublisher) -> Self::T {
        let events = RemoteEventSender::new(&self.namespace, publisher);
        let exit = self.exit.clone();
        Local::<I>::new(events, exit, &self.namespace, &self.containerd_address)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(level = "Info"))]
    async fn delete_shim(&mut self) -> shim::Result<api::DeleteResponse> {
        Ok(api::DeleteResponse {
            exit_status: 137,
            exited_at: Some(Utc::now().to_timestamp()).into(),
            ..Default::default()
        })
    }
}
