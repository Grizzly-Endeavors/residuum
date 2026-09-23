//! The workbench artifacts listener: serves artifacts on their own origin.
//!
//! Artifacts are agent-written pages that load third-party scripts, so they must
//! never share the web UI's origin, where they could call the whole API
//! directly. They get a separate listener instead, on the first free port
//! after the gateway's. Locally that is a different origin
//! (`localhost:7702` beside the UI on `localhost:7700`); through the relay,
//! requests for `{user}.workbench.<relay>` are tagged by the relay and routed
//! here by the tunnel client. Being a real origin, artifacts can be folders with
//! relative scripts, modules, and workers, and can use browser storage.
//!
//! The listener is read-only: `GET`/`HEAD` of artifact files, nothing else. Artifacts
//! reach Residuum's API only through the web UI's bridge.
//!
//! URL layout: `/{artifact}/` is the artifact's page, `/{artifact}/{path}` a file in a
//! folder artifact, and `/{artifact}` redirects to `/{artifact}/` so relative URLs resolve
//! inside the artifact.

use std::path::PathBuf;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;

use super::{ArtifactFileError, discover_artifacts, is_valid_artifact_name, read_artifact_file};

/// How many ports after the gateway's are tried before giving up.
const PORT_SEARCH_ATTEMPTS: u16 = 10;

/// Whether the artifacts listener is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkbenchServing {
    /// Serving on this port, beside the gateway.
    Running { port: u16 },
    /// Not serving; the reason is shown to the user.
    Unavailable { reason: String },
}

impl WorkbenchServing {
    /// The listener's port, when it is running.
    #[must_use]
    pub(crate) fn port(&self) -> Option<u16> {
        match self {
            Self::Running { port } => Some(*port),
            Self::Unavailable { .. } => None,
        }
    }
}

/// Start the artifacts listener for `dir` beside the gateway on `bind`. Returns
/// whether it is serving and, when it is, the switch that stops it.
///
/// Failing to bind is not fatal: Residuum runs without the workbench and the
/// web UI shows why (the reason is also logged at `error`). The listener runs
/// as a monitored task, so a panic or serve failure is logged too.
pub(crate) async fn start(
    bind: &str,
    gateway_port: u16,
    reserved: &[u16],
    dir: PathBuf,
) -> (WorkbenchServing, Option<tokio::sync::watch::Sender<bool>>) {
    let (listener, port) = match bind_listener(bind, gateway_port, reserved).await {
        Ok(bound) => bound,
        Err(reason) => {
            tracing::error!(%reason, gateway_port, "workbench artifacts listener could not start; workbench artifacts are unavailable");
            return (
                WorkbenchServing::Unavailable {
                    reason: format!(
                        "Residuum couldn't start the workbench artifacts listener: {reason}."
                    ),
                },
                None,
            );
        }
    };
    tracing::info!(addr = %format!("{bind}:{port}"), "workbench artifacts listening");

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let app = router(dir);
    crate::util::spawn_monitored("workbench-listener", async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx.wait_for(|stop| *stop).await.ok();
            })
            .await
        {
            tracing::error!(error = %e, "workbench artifacts listener failed");
        }
    });
    (WorkbenchServing::Running { port }, Some(shutdown_tx))
}

/// Bind the artifacts listener on the first free port after `gateway_port`,
/// skipping every port in `reserved` (another listener's configured port,
/// such as Teams', even when that listener isn't running yet). The choice is
/// stable across restarts while the machine's ports don't change, which keeps
/// the artifacts' origin, and so their browser storage, stable too.
///
/// # Errors
/// Returns a plain-language reason when no port could be bound.
pub(crate) async fn bind_listener(
    bind: &str,
    gateway_port: u16,
    reserved: &[u16],
) -> Result<(tokio::net::TcpListener, u16), String> {
    let mut last_error = None;
    for offset in 1..=PORT_SEARCH_ATTEMPTS {
        let Some(port) = gateway_port.checked_add(offset) else {
            break;
        };
        if reserved.contains(&port) {
            continue;
        }
        match tokio::net::TcpListener::bind((bind, port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                last_error = Some(format!("port {port} is in use"));
            }
            Err(e) => return Err(format!("couldn't listen on {bind}:{port}: {e}")),
        }
    }
    Err(format!(
        "no free port in the {PORT_SEARCH_ATTEMPTS} after {gateway_port} ({})",
        last_error.unwrap_or_else(|| "all reserved".to_string())
    ))
}

/// Router for the artifacts listener, serving artifacts from `dir`.
pub(crate) fn router(dir: PathBuf) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/{name}", get(artifact_root))
        .route("/{name}/", get(artifact_index))
        .route("/{name}/{*rest}", get(artifact_file))
        .fallback(|| async { not_found("Nothing here.") })
        .layer(axum::middleware::from_fn(
            crate::gateway::cross_site::reject_cross_site_requests,
        ))
        .with_state(dir)
}

async fn home() -> Response {
    page(
        StatusCode::OK,
        "Workbench artifacts open from the Workbench page in Residuum.",
    )
}

/// `/{artifact}` → `/{artifact}/`, so relative URLs in the artifact resolve inside it.
async fn artifact_root(State(dir): State<PathBuf>, Path(name): Path<String>, uri: Uri) -> Response {
    let exists = is_valid_artifact_name(&name)
        && discover_artifacts(&dir)
            .await
            .is_ok_and(|artifacts| artifacts.iter().any(|t| t.name == name));
    if !exists {
        return not_found(&format!("There's no workbench artifact named \"{name}\"."));
    }
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    Redirect::permanent(&format!("/{name}/{query}")).into_response()
}

async fn artifact_index(State(dir): State<PathBuf>, Path(name): Path<String>) -> Response {
    serve(&dir, &name, "").await
}

async fn artifact_file(
    State(dir): State<PathBuf>,
    Path((name, rest)): Path<(String, String)>,
) -> Response {
    serve(&dir, &name, &rest).await
}

async fn serve(dir: &std::path::Path, name: &str, rest: &str) -> Response {
    if !is_valid_artifact_name(name) {
        return not_found(&format!("There's no workbench artifact named \"{name}\"."));
    }
    match read_artifact_file(dir, name, rest).await {
        Ok(served) => {
            let mut response = served.bytes.into_response();
            let headers = response.headers_mut();
            if let Ok(value) = HeaderValue::from_str(&served.content_type) {
                headers.insert(header::CONTENT_TYPE, value);
            }
            // Artifacts reload live while the agent edits them.
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            response
        }
        Err(ArtifactFileError::NoSuchArtifact(_)) => {
            not_found(&format!("There's no workbench artifact named \"{name}\"."))
        }
        Err(ArtifactFileError::NotFound { .. }) => {
            not_found(&format!("The artifact \"{name}\" has no file \"{rest}\"."))
        }
        Err(e @ ArtifactFileError::TooLarge { .. }) => {
            page(StatusCode::PAYLOAD_TOO_LARGE, &e.to_string())
        }
        Err(e @ ArtifactFileError::Io { .. }) => {
            tracing::error!(artifact = %name, path = %rest, error = %e, "failed to serve workbench artifact file");
            page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Couldn't read this artifact's files. Residuum's logs have the details.",
            )
        }
    }
}

fn not_found(message: &str) -> Response {
    page(StatusCode::NOT_FOUND, message)
}

/// A minimal HTML page, since these responses land in the artifact frame.
fn page(status: StatusCode, message: &str) -> Response {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Workbench</title><body style=\"font:14px sans-serif;color:#a8a29e;background:#12100e;padding:2rem\"><p>{escaped}</p></body>"
    );
    let mut resp = (status, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    async fn get_path(dir: &std::path::Path, path: &str) -> Response {
        router(dir.to_path_buf())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_text(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn serves_folder_artifacts_with_relative_files() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("graph/index.html"),
            "<head></head><script src=app.js></script>",
        );
        write(&dir.path().join("graph/app.js"), "console.log(1)");

        let index = get_path(dir.path(), "/graph/").await;
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(
            index.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert!(body_text(index).await.contains("window.residuum"));

        let script = get_path(dir.path(), "/graph/app.js").await;
        assert_eq!(script.status(), StatusCode::OK);
        assert_eq!(body_text(script).await, "console.log(1)");
    }

    #[tokio::test]
    async fn bare_artifact_path_redirects_to_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("chart.html"), "<head></head>");
        let resp = get_path(dir.path(), "/chart?x=1").await;
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/chart/?x=1");

        let missing = get_path(dir.path(), "/nope").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn escapes_and_writes_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("graph/index.html"), "x");
        write(&dir.path().join("graph.state.json"), "{}");

        for path in [
            "/graph/..%2Fgraph.state.json",
            "/graph/%2e%2e/graph.state.json",
            "/Bad.Name/",
        ] {
            let resp = get_path(dir.path(), path).await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
        }

        let post = router(dir.path().to_path_buf())
            .oneshot(Request::post("/graph/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn bind_listener_skips_reserved_and_busy_ports() {
        let busy = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let busy_port = busy.local_addr().unwrap().port();
        let gateway_port = busy_port - 1;
        let reserved = busy_port + 1;

        let (listener, port) = bind_listener("127.0.0.1", gateway_port, &[reserved])
            .await
            .unwrap();
        assert!(
            port > reserved,
            "skips the busy port and the reserved one, got {port}"
        );
        assert_eq!(listener.local_addr().unwrap().port(), port);
    }
}
