// Heavily inspired by wasmtime serve command

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Result};
use wasmtime::component::ResourceTable;
use wasmtime::Store;
use wasmtime_wasi::{self as wasi_preview2};
use wasmtime_wasi_http::bindings::http::types::Scheme;
use wasmtime_wasi_http::bindings::ProxyPre;
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wasmtime_wasi_http::io::TokioIo;
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::instance::WasiCtx;

const DEFAULT_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)), 8080);

const DEFAULT_BACKLOG: u32 = 100;

fn wasi_store_for_request(handler: &ProxyHandlerInner, req_id: u64) -> Store<WasiCtx> {
    let engine = handler.instance_pre.engine();
    let mut builder = wasi_preview2::WasiCtxBuilder::new();

    builder.envs(&handler.env);
    builder.env("REQUEST_ID", req_id.to_string());

    let ctx = WasiCtx {
        wasi_preview1: None,
        wasi_preview2_cli: builder.build(),
        wasi_preview2_http: WasiHttpCtx::new(),
        resource_table: ResourceTable::default(),
        envs: vec![],
    };

    Store::new(engine, ctx)
}

struct ProxyHandlerInner {
    instance_pre: ProxyPre<WasiCtx>,
    next_id: AtomicU64,
    env: Vec<(String, String)>,
}

impl ProxyHandlerInner {
    fn next_req_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Clone)]
struct ProxyHandler(Arc<ProxyHandlerInner>);

impl ProxyHandler {
    fn new(instance_pre: ProxyPre<WasiCtx>, env: &[(String, String)]) -> Self {
        Self(Arc::new(ProxyHandlerInner {
            instance_pre,
            env: env.to_owned(),
            next_id: AtomicU64::from(0),
        }))
    }
}

type Request = hyper::Request<hyper::body::Incoming>;

pub(crate) async fn serve_conn(instance: ProxyPre<WasiCtx>, store: Store<WasiCtx>) -> Result<()> {
    use hyper::server::conn::http1;
    let env = store.data().envs();

    let addr = env
        .iter()
        .find(|(key, _)| key == "WASMTIME_HTTP_PROXY_SOCKET_ADDR")
        .and_then(|(_, val)| val.parse().ok())
        .unwrap_or(DEFAULT_ADDR);

    let backlog = env
        .iter()
        .find(|(key, _)| key == "WASMTIME_HTTP_PROXY_BACKLOG")
        .and_then(|(_, val)| val.parse().ok())
        .unwrap_or(DEFAULT_BACKLOG);

    let socket = match addr {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
    };

    // Conditionally enable `SO_REUSEADDR` depending on the current
    // platform. On Unix we want this to be able to rebind an address in
    // the `TIME_WAIT` state which can happen then a server is killed with
    // active TCP connections and then restarted. On Windows though if
    // `SO_REUSEADDR` is specified then it enables multiple applications to
    // bind the port at the same time which is not something we want. Hence
    // this is conditionally set based on the platform (and deviates from
    // Tokio's default from always-on).
    socket.set_reuseaddr(!cfg!(windows))?;
    socket.bind(addr)?;

    let listener = socket.listen(backlog)?;

    log::info!("Serving HTTP on http://{}/", listener.local_addr()?);

    let handler = ProxyHandler::new(instance, env);

    loop {
        let (stream, _) = listener.accept().await?;
        log::debug!("New connection");

        let stream = TokioIo::new(stream);
        let h = handler.clone();

        tokio::spawn(async {
            if let Err(e) = http1::Builder::new()
                .keep_alive(true)
                .serve_connection(
                    stream,
                    hyper::service::service_fn(move |req| handle_request(h.clone(), req)),
                )
                .await
            {
                log::error!("error: {e:?}");
            }
        });
    }
}

async fn handle_request(
    ProxyHandler(inner): ProxyHandler,
    req: Request,
) -> Result<hyper::Response<HyperOutgoingBody>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();

    let req_id = inner.next_req_id();

    log::info!(
        "Request {req_id} handling {} to {}",
        req.method(),
        req.uri()
    );

    let mut store = wasi_store_for_request(&inner, req_id);

    let req = store.data_mut().new_incoming_request(Scheme::Http, req)?;
    let out = store.data_mut().new_response_outparam(sender)?;
    let proxy = inner.instance_pre.instantiate_async(&mut store).await?;

    let task = tokio::spawn(async move {
        if let Err(e) = proxy
            .wasi_http_incoming_handler()
            .call_handle(store, req, out)
            .await
        {
            log::error!("[{req_id}] :: {:#?}", e);
            return Err(e);
        }

        Ok(())
    });

    match receiver.await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => {
            // An error in the receiver (`RecvError`) only indicates that the
            // task exited before a response was sent (i.e., the sender was
            // dropped); it does not describe the underlying cause of failure.
            // Instead we retrieve and propagate the error from inside the task
            // which should more clearly tell the user what went wrong. Note
            // that we assume the task has already exited at this point so the
            // `await` should resolve immediately.
            let e = match task.await {
                Ok(e) => e.expect_err("if the receiver has an error, the task must have failed"),
                Err(e) => e.into(),
            };

            bail!("guest never invoked `response-outparam::set` method: {e:?}")
        }
    }
}
