use wasmtime::Engine;
use containerd_shim::{self as shim, api, Task, TtrpcContext, TtrpcResult, Error as ShimError, ExitSignal};
use ttrpc::Code;
use std::collections::{HashSet,HashMap};
use thiserror::Error;
use anyhow::{Result, Error as AnyError, Context};
use std::env;
use std::path::Path;
use std::fs::File;
use oci_spec::runtime;
use serde_json as json;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Shim(#[from] ShimError),
    #[error("{0}")]
    Other(#[from] AnyError),
}

#[derive(Clone)]
pub struct Local {
	engine: Engine,
	sandboxes: HashSet<std::string::String>,
	exit: ExitSignal,
}

impl Local {
	fn is_first(&self) -> bool {
		self.sandboxes.is_empty()
	}

	pub fn new(engine: Engine) -> Self {
		Local {
			engine,
			sandboxes: HashSet::new(),
			exit: ExitSignal::default(),
		}
	}
}

impl Task for Local {
	fn create(&self, _ctx: &TtrpcContext, _req: api::CreateTaskRequest) -> TtrpcResult<api::CreateTaskResponse> {
        Err(::ttrpc::Error::RpcStatus(::ttrpc::get_status(Code::UNIMPLEMENTED, "/containerd.task.v2.Task/Create is not supported".to_string())))
    }
}


impl shim::Shim for Local {
    type T = Local;
    type Error = Error;

    fn new(
        _runtime_id: &str,
        _id: &str,
        _namespace: &str,
        _publisher: shim::RemotePublisher,
        _config: &mut shim::Config,
    ) -> Self {
        let cfg = wasmtime::Config::default();
		let engine = wasmtime::Engine::new(&cfg).unwrap();
        return Local::new(engine);
    }

    fn start_shim(&mut self, opts: shim::StartOpts) -> Result<String, Error> {
        let cwd = env::current_dir().context("Could not determine working directory")?;
        let p = Path::new(&cwd).join("config.json");

        let rdr =
            File::open(p.to_str().unwrap_or("")).context("Failed to open runtime spec")?;
        let cfg: runtime::Spec =
            json::from_reader(rdr).context("Error parsing runtime spec json")?;

        let default_annotations: HashMap<std::string::String, std::string::String> =
            HashMap::new();
        let default_group = std::string::String::new();
        let grouping = cfg
            .annotations()
            .as_ref()
            .unwrap_or_else(|| &default_annotations)
            .get("io.kubernetes.cri.sandbox-id")
            .unwrap_or(&default_group);

		// TODO: Currently using a fork of containerd-shim to get the "grouping"
		// It is added in a hacky way and would be nice to upstream *something*.

        let opts2 = shim::StartOpts {
            id: opts.id.clone(),
            publish_binary: opts.publish_binary.clone(),
            address: opts.address.clone(),
            ttrpc_address: opts.ttrpc_address.clone(),
            namespace: opts.namespace.clone(),
            grouping: grouping.clone(),
        };

        let address = shim::spawn(opts2, Vec::new())?;
        Ok(address)
    }

    fn wait(&mut self) {
        self.exit.wait();
    }

    fn get_task_service(&self) -> Self::T {
        self.clone()
    }
}
