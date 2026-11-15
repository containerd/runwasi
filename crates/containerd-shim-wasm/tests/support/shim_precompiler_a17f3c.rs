use std::hash::Hash;
use std::sync::{Arc, Mutex};

use crate::sandbox::Sandbox;
use crate::sandbox::context::{RuntimeContext, WasmLayer};
use crate::shim::cache::precompiler;
use crate::shim::{Compiler, Shim};
use anyhow::Result;
use oci_spec::image::{Descriptor, Digest, MediaType};
use oci_spec::runtime::Spec;
use tempfile::TempDir;

type Calls = Arc<Mutex<Vec<Vec<Vec<u8>>>>>;

#[derive(Clone, Default)]
struct CountingCompiler {
    calls: Calls,
}

impl Compiler for CountingCompiler {
    fn cache_key(&self) -> impl Hash {
        "v1"
    }

    async fn compile(&self, layers: &[WasmLayer]) -> Result<Vec<Option<Vec<u8>>>> {
        self.calls
            .lock()
            .unwrap()
            .push(layers.iter().map(|layer| layer.layer.clone()).collect());
        Ok(layers
            .iter()
            .map(|layer| Some(compiled(&layer.layer)))
            .collect())
    }
}

fn compiled(bytes: &[u8]) -> Vec<u8> {
    [b"compiled:".as_slice(), bytes].concat()
}

fn layer(bytes: &[u8]) -> WasmLayer {
    WasmLayer {
        config: Descriptor::new(
            MediaType::Other("application/wasm".to_string()),
            bytes.len() as u64,
            Digest::try_from(format!("sha256:{:064x}", 0)).unwrap(),
        ),
        precompiled: false,
        layer: bytes.to_vec(),
    }
}

fn with_cache_dir<R>(dir: Option<&str>, work: impl FnOnce() -> R) -> R {
    temp_env::with_vars(
        [
            ("RUNWASI_MODULE_CACHE_DIR", dir),
            ("RUNWASI_MODULE_CACHE_TTL_SECS", None),
            ("RUNWASI_MODULE_CACHE_MAX_BYTES", None),
        ],
        work,
    )
}

fn block_on<F: std::future::Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(work)
}

#[derive(Default)]
struct UnusedSandbox;

impl Sandbox for UnusedSandbox {
    async fn run_wasi(&self, _ctx: &impl RuntimeContext) -> Result<i32> {
        unreachable!("the shim is only asked for its compiler")
    }
}

static OVERRIDING: std::sync::OnceLock<Calls> = std::sync::OnceLock::new();
static DEFAULTING: std::sync::OnceLock<Calls> = std::sync::OnceLock::new();

/// A shim that answers `compiler_with_spec` itself.
struct ReadsTheSpec;

/// A shim that leaves `compiler_with_spec` alone, so its compiler arrives
/// through the path a shim gets by default.
struct LeavesTheSpecAlone;

/// A shim with nothing to offer.
struct OffersNoCompiler;

impl Shim for ReadsTheSpec {
    type Sandbox = UnusedSandbox;

    fn name() -> &'static str {
        "reads-the-spec"
    }

    async fn compiler_with_spec(_spec: &Spec) -> Option<impl Compiler> {
        Some(CountingCompiler {
            calls: OVERRIDING.get_or_init(Arc::default).clone(),
        })
    }
}

impl Shim for LeavesTheSpecAlone {
    type Sandbox = UnusedSandbox;

    fn name() -> &'static str {
        "leaves-the-spec-alone"
    }

    async fn compiler() -> Option<impl Compiler> {
        Some(CountingCompiler {
            calls: DEFAULTING.get_or_init(Arc::default).clone(),
        })
    }
}

impl Shim for OffersNoCompiler {
    type Sandbox = UnusedSandbox;

    fn name() -> &'static str {
        "offers-no-compiler"
    }
}

#[test]
fn the_precompiler_a_shim_runs_with_is_cached() {
    let dir = TempDir::new().unwrap();
    let spec = Spec::default();

    with_cache_dir(dir.path().to_str(), || {
        block_on(async {
            let compiler = precompiler::<ReadsTheSpec>(&spec)
                .await
                .expect("the shim offers a compiler");

            let first = compiler.compile(&[layer(b"aaa")]).await.unwrap();
            let second = compiler.compile(&[layer(b"aaa")]).await.unwrap();

            assert_eq!(first, vec![Some(compiled(b"aaa"))]);
            assert_eq!(second, first);
        });
    });

    let bundle = TempDir::new().unwrap();
    spec.save(bundle.path().join("config.json")).unwrap();
    with_cache_dir(dir.path().to_str(), || {
        block_on(containerd_stub::construct_twice(bundle.path()));
    });

    let calls = OVERRIDING.get().expect("the shim was asked");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![vec![b"aaa".to_vec()], vec![b"ccc".to_vec()]],
        "both the helper and real instance construction must reuse their cached layers"
    );
}

#[test]
fn the_precompiler_follows_whatever_the_shim_offers() {
    let dir = TempDir::new().unwrap();
    let spec = Spec::default();

    with_cache_dir(dir.path().to_str(), || {
        block_on(async {
            let compiler = precompiler::<LeavesTheSpecAlone>(&spec)
                .await
                .expect("this shim offers a compiler");
            compiler.compile(&[layer(b"bbb")]).await.unwrap();
            let served = compiler.compile(&[layer(b"bbb")]).await.unwrap();

            assert_eq!(served, vec![Some(compiled(b"bbb"))]);
            assert!(
                precompiler::<OffersNoCompiler>(&spec).await.is_none(),
                "a shim with nothing to offer is given nothing to wrap"
            );
        });
    });

    let calls = DEFAULTING.get().expect("the shim was asked");
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "a shim that never touches the spec is cached just the same"
    );

    let blocked = dir.path().join("not-a-directory");
    std::fs::write(&blocked, b"occupied").unwrap();
    let unavailable = blocked.join("cache");
    for selected in [None, unavailable.to_str()] {
        with_cache_dir(selected, || {
            block_on(async {
                let before = calls.lock().unwrap().len();
                let compiler = precompiler::<LeavesTheSpecAlone>(&spec)
                    .await
                    .expect("an unavailable cache must not remove the shim's compiler");
                for _ in 0..2 {
                    assert_eq!(
                        compiler.compile(&[layer(b"bbb")]).await.unwrap(),
                        vec![Some(compiled(b"bbb"))]
                    );
                }
                assert_eq!(
                    calls.lock().unwrap().len(),
                    before + 2,
                    "fallback forwards both repeated requests"
                );
                assert!(precompiler::<OffersNoCompiler>(&spec).await.is_none());
            });
        });
    }
}

mod containerd_stub {
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use containerd_client::services::v1::*;
    use containerd_client::tonic::{self, Request, Response, Status};
    use tonic::codegen::{Body, BoxFuture, Service, StdError, http};

    #[derive(Clone)]
    pub struct Rpc<const SERVICE: u8>(pub Arc<tokio::sync::Notify>);

    impl<const SERVICE: u8> tonic::server::NamedService for Rpc<SERVICE> {
        const NAME: &'static str = match SERVICE {
            0 => "containerd.services.containers.v1.Containers",
            1 => "containerd.services.images.v1.Images",
            2 => "containerd.services.content.v1.Content",
            _ => "containerd.services.leases.v1.Leases",
        };
    }

    // These protocol responses describe an image whose content store has only
    // the original layer. Each construction must ask its precompiler for it.
    struct GetContainer;
    impl tonic::server::UnaryService<GetContainerRequest> for GetContainer {
        type Response = GetContainerResponse;
        type Future = BoxFuture<Response<Self::Response>, Status>;
        fn call(&mut self, _: Request<GetContainerRequest>) -> Self::Future {
            Box::pin(async {
                Ok(Response::new(GetContainerResponse {
                    container: Some(Container {
                        image: "image".into(),
                        ..Default::default()
                    }),
                }))
            })
        }
    }

    struct GetImage;
    impl tonic::server::UnaryService<GetImageRequest> for GetImage {
        type Response = GetImageResponse;
        type Future = BoxFuture<Response<Self::Response>, Status>;
        fn call(&mut self, _: Request<GetImageRequest>) -> Self::Future {
            Box::pin(async {
                Ok(Response::new(GetImageResponse {
                    image: Some(Image {
                        name: "image".into(),
                        target: Some(containerd_client::types::Descriptor {
                            digest: digest(1),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                }))
            })
        }
    }

    struct GetInfo;
    impl tonic::server::UnaryService<InfoRequest> for GetInfo {
        type Response = InfoResponse;
        type Future = BoxFuture<Response<Self::Response>, Status>;
        fn call(&mut self, request: Request<InfoRequest>) -> Self::Future {
            Box::pin(async move {
                Ok(Response::new(InfoResponse {
                    info: Some(Info {
                        digest: request.into_inner().digest,
                        ..Default::default()
                    }),
                }))
            })
        }
    }

    fn digest(n: u8) -> String {
        format!("sha256:{n:064x}")
    }

    struct Read;
    impl tonic::server::ServerStreamingService<ReadContentRequest> for Read {
        type Response = ReadContentResponse;
        type ResponseStream = tonic::codegen::BoxStream<Self::Response>;
        type Future = BoxFuture<Response<Self::ResponseStream>, Status>;
        fn call(&mut self, request: Request<ReadContentRequest>) -> Self::Future {
            Box::pin(async move {
                let requested = request.into_inner().digest;
                let data = if requested == digest(1) {
                    serde_json::to_vec(&serde_json::json!({
                        "schemaVersion": 2,
                        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": digest(2), "size": 0},
                        "layers": [{"mediaType": "application/wasm", "digest": digest(3), "size": 3}]
                    })).unwrap()
                } else if requested == digest(2) {
                    br#"{"architecture":"wasm","os":"wasi"}"#.to_vec()
                } else if requested == digest(3) {
                    b"ccc".to_vec()
                } else {
                    return Err(Status::not_found("unknown fixture content"));
                };
                let stream: Self::ResponseStream =
                    Box::pin(tokio_stream::iter([Ok(ReadContentResponse {
                        offset: 0,
                        data,
                    })]));
                Ok(Response::new(stream))
            })
        }
    }

    impl<B, const SERVICE: u8> Service<http::Request<B>> for Rpc<SERVICE>
    where
        B: Body + Send + 'static,
        B::Error: Into<StdError> + Send + 'static,
    {
        type Response = http::Response<tonic::body::BoxBody>;
        type Error = std::convert::Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, request: http::Request<B>) -> Self::Future {
            let reached = self.0.clone();
            Box::pin(async move {
                let response = match request.uri().path() {
                    "/containerd.services.containers.v1.Containers/Get" => {
                        tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                            .unary(GetContainer, request)
                            .await
                    }
                    "/containerd.services.images.v1.Images/Get" => {
                        tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                            .unary(GetImage, request)
                            .await
                    }
                    "/containerd.services.content.v1.Content/Info" => {
                        tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                            .unary(GetInfo, request)
                            .await
                    }
                    "/containerd.services.content.v1.Content/Read" => {
                        tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                            .server_streaming(Read, request)
                            .await
                    }
                    "/containerd.services.leases.v1.Leases/Create" => {
                        // Compilation has returned and the client is about to
                        // persist its output. Stop here, before container setup.
                        reached.notify_one();
                        std::future::pending().await
                    }
                    _ => Status::unimplemented("unused fixture method").into_http(),
                };
                Ok(response)
            })
        }
    }

    pub async fn construct_twice(bundle: &std::path::Path) {
        use crate::shim::Instance;
        use containerd_shimkit::sandbox::{Instance as _, InstanceConfig};

        let socket = bundle.join("containerd.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let incoming = futures::stream::unfold(listener, |listener| async {
            Some((listener.accept().await.map(|(stream, _)| stream), listener))
        });
        let reached = Arc::new(tokio::sync::Notify::new());
        let server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(Rpc::<0>(reached.clone()))
                .add_service(Rpc::<1>(reached.clone()))
                .add_service(Rpc::<2>(reached.clone()))
                .add_service(Rpc::<3>(reached.clone()))
                .serve_with_incoming(incoming),
        );
        let cfg = InstanceConfig {
            bundle: bundle.to_path_buf(),
            containerd_address: socket.to_str().unwrap().into(),
            namespace: "cache-test".into(),
            ..Default::default()
        };
        for id in ["first", "second"] {
            let cfg = cfg.clone();
            let construction =
                tokio::spawn(
                    async move { Instance::<super::ReadsTheSpec>::new(id.into(), &cfg).await },
                );
            let arrived =
                tokio::time::timeout(std::time::Duration::from_secs(20), reached.notified()).await;
            construction.abort();
            let stopped = construction.await;
            assert!(
                arrived.is_ok(),
                "construction never reached compiled-content storage: {}",
                match stopped {
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => error.to_string(),
                    _ => "construction ended early".into(),
                }
            );
        }
        server.abort();
        let _ = server.await;
    }
}
