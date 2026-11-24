use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::header::{self, HeaderValue};
use axum::http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::server::Acceptor;
use tower::ServiceExt;
use tracing::{debug, warn};

use super::proxy::{InstanceProxy, InstanceTargets, text_response};
use super::tls::CertResolver;
use super::types::{HostKind, Hostnames, normalize_host};
use crate::browser_landing::{self, Landing};
use crate::pairing::remote::RemoteTransport;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const H1_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
const H2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const H2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";

/// Headers only Residuum's own components may set. A browser-supplied copy
/// would let it pose as an authenticated A2A caller. Forwarding headers go
/// too: the peer address is the one the engine was given, not one the peer
/// names.
const INTERNAL_HEADERS: [&str; 3] = ["x-residuum-a2a-caller", "x-real-ip", "x-forwarded-for"];

pub(crate) struct EngineRouters {
    pub(crate) ui: Router,
    pub(crate) workbench: Router,
}

pub(crate) struct EngineDeps {
    pub(crate) resolver: Arc<CertResolver>,
    pub(crate) hostnames: watch::Receiver<Option<Hostnames>>,
    pub(crate) routers: EngineRouters,
    pub(crate) sibling_routes: Router,
    pub(crate) a2a_port: Option<u16>,
    pub(crate) teams_ports: watch::Receiver<BTreeMap<String, u16>>,
}

pub(crate) struct Engine {
    resolver: Arc<CertResolver>,
    hostnames: watch::Receiver<Option<Hostnames>>,
    routers: EngineRouters,
    sibling_routes: Router,
    a2a_port: Option<u16>,
    teams_ports: watch::Receiver<BTreeMap<String, u16>>,
    proxy: InstanceProxy,
    server_config: Mutex<Option<Arc<ServerConfig>>>,
}

impl Engine {
    pub(crate) fn new(deps: EngineDeps) -> Arc<Self> {
        Arc::new(Self {
            resolver: deps.resolver,
            hostnames: deps.hostnames,
            routers: deps.routers,
            sibling_routes: deps.sibling_routes,
            a2a_port: deps.a2a_port,
            teams_ports: deps.teams_ports,
            proxy: InstanceProxy::new(),
            server_config: Mutex::new(None),
        })
    }

    pub(crate) async fn serve<S>(self: &Arc<Self>, relay_host: String, peer_ip: String, io: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let Some(tls) = self.accept_tls(&relay_host, &peer_ip, io).await else {
            return;
        };
        if tls.get_ref().1.alpn_protocol() == Some(ACME_TLS_ALPN) {
            let mut tls = tls;
            if let Err(e) = tls.shutdown().await {
                debug!(error = %e, "closing an ACME validation connection failed");
            }
            return;
        }

        let engine = Arc::clone(self);
        let service = service_fn(move |req: Request<Incoming>| {
            let engine = Arc::clone(&engine);
            let peer_ip = peer_ip.clone();
            async move { Ok::<_, Infallible>(engine.handle(req.map(Body::new), &peer_ip).await) }
        });

        let mut builder = auto::Builder::new(TokioExecutor::new());
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(H1_HEADER_READ_TIMEOUT);
        builder
            .http2()
            .timer(TokioTimer::new())
            .keep_alive_interval(H2_KEEP_ALIVE_INTERVAL)
            .keep_alive_timeout(H2_KEEP_ALIVE_TIMEOUT);
        if let Err(e) = builder
            .serve_connection_with_upgrades(TokioIo::new(tls), service)
            .await
        {
            debug!(error = %e, "a remote HTTP connection ended with an error");
        }
    }

    async fn accept_tls<S>(
        &self,
        relay_host: &str,
        peer_ip: &str,
        io: S,
    ) -> Option<tokio_rustls::server::TlsStream<S>>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let handshake = async {
            let start = LazyConfigAcceptor::new(Acceptor::default(), io)
                .await
                .map_err(|e| format!("reading the TLS ClientHello failed: {e}"))?;
            let sni = start
                .client_hello()
                .server_name()
                .map(normalize_host)
                .ok_or_else(|| "the TLS ClientHello named no server".to_string())?;
            let relay_host = normalize_host(relay_host);
            let hostnames = self.hostnames.borrow().clone();
            let Some(hostnames) = hostnames else {
                return Err("this instance has no identity yet".to_string());
            };
            if sni != relay_host || hostnames.kind_of(&sni).is_none() {
                warn!(
                    sni = %sni,
                    relay_host = %relay_host,
                    peer_ip,
                    "closed a remote connection whose TLS server name isn't one of this instance's names"
                );
                return Ok(None);
            }
            let config = self
                .server_config()
                .map_err(|e| format!("couldn't build the TLS configuration: {e:#}"))?;
            let tls = start
                .into_stream(config)
                .await
                .map_err(|e| format!("the TLS handshake failed: {e}"))?;
            Ok(Some(tls))
        };
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await {
            Ok(Ok(tls)) => tls,
            Ok(Err(reason)) => {
                debug!(peer_ip, reason, "closed a remote connection before HTTP");
                None
            }
            Err(_) => {
                debug!(
                    peer_ip,
                    timeout_secs = HANDSHAKE_TIMEOUT.as_secs(),
                    "closed a remote connection that didn't finish its TLS handshake in time"
                );
                None
            }
        }
    }

    fn server_config(&self) -> anyhow::Result<Arc<ServerConfig>> {
        let mut cached = self
            .server_config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = cached.as_ref() {
            return Ok(Arc::clone(config));
        }
        let config = self.resolver.server_config()?;
        *cached = Some(Arc::clone(&config));
        Ok(config)
    }

    fn ui_origin(&self) -> Option<String> {
        self.hostnames
            .borrow()
            .as_ref()
            .map(|names| format!("https://{}", names.ui))
    }

    async fn handle(&self, mut req: Request<Body>, peer_ip: &str) -> Response<Body> {
        for name in INTERNAL_HEADERS {
            req.headers_mut().remove(name);
        }

        let Some(host) = request_host(&mut req) else {
            return misdirected(&req, self.ui_origin().as_deref());
        };
        let kind = self
            .hostnames
            .borrow()
            .as_ref()
            .and_then(|names| names.kind_of(&host));
        let host = normalize_host(&host);

        match kind {
            Some(HostKind::Ui) => call_router(&self.routers.ui, req, peer_ip, &host).await,
            Some(HostKind::Workbench) => {
                call_router(&self.routers.workbench, req, peer_ip, &host).await
            }
            Some(HostKind::Instance) if is_sibling_path(req.uri().path()) => {
                call_router(&self.sibling_routes, req, peer_ip, &host).await
            }
            Some(HostKind::Instance) => {
                let teams_ports = self.teams_ports.borrow().clone();
                self.proxy
                    .handle(
                        req,
                        peer_ip,
                        InstanceTargets {
                            a2a_port: self.a2a_port,
                            teams_ports: &teams_ports,
                            ui_origin: self.ui_origin(),
                        },
                    )
                    .await
            }
            None => {
                debug!(host = %host, peer_ip, "refused a request for a host this instance doesn't serve");
                misdirected(&req, self.ui_origin().as_deref())
            }
        }
    }
}

fn is_sibling_path(path: &str) -> bool {
    path == crate::remote_access::siblings::protocol::REQUEST_PATH
        || path.starts_with("/_sibling/join/")
}

fn request_host(req: &mut Request<Body>) -> Option<String> {
    if let Some(authority) = req.uri().authority().map(|a| a.as_str().to_owned()) {
        let value = HeaderValue::from_str(&authority).ok()?;
        req.headers_mut().insert(header::HOST, value);
        return Some(authority);
    }
    req.headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

async fn call_router(
    router: &Router,
    mut req: Request<Body>,
    peer_ip: &str,
    host: &str,
) -> Response<Body> {
    req.extensions_mut().insert(RemoteTransport {
        peer_ip: Some(peer_ip.to_string()),
        origin: Some(format!("https://{host}")),
    });
    match router.clone().oneshot(req).await {
        Ok(response) => response,
        Err(infallible) => match infallible {},
    }
}

fn misdirected(req: &Request<Body>, ui_origin: Option<&str>) -> Response<Body> {
    const MESSAGE: &str = "This address isn't served by this Residuum instance.";
    if browser_landing::is_navigation(req.method(), req.headers()) {
        return browser_landing::fallback_page(
            StatusCode::MISDIRECTED_REQUEST,
            &Landing::new("Address not served here", MESSAGE)
                .with_app_link(ui_origin, "Open Residuum"),
        );
    }
    text_response(StatusCode::MISDIRECTED_REQUEST, MESSAGE)
}
