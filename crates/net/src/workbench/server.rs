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
//! Artifact files are read-only: `GET`/`HEAD` of pages and the files they load.
//! Everything under `/api` is forwarded in-process to the hub router (see
//! [`super::forward`]), so a page on this origin reaches Residuum's API and
//! sockets directly.
//!
//! URL layout: `/{artifact}/` is the artifact's page, `/{artifact}/{path}` a file in a
//! folder artifact, `/{artifact}` redirects to `/{artifact}/` so relative URLs resolve
//! inside the artifact, and `/api/...` is the API.

use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use tokio_util::io::ReaderStream;

use super::forward::{self, HubApi};
use super::{
    ArtifactBody, ArtifactFileError, SdkContext, discover_artifacts, is_valid_artifact_name,
    read_artifact_file,
};
use crate::browser_landing::{self, AppNotice, Landing};
use crate::pairing::HANDOFF_PAGE_PATH;

/// How many ports after the gateway's are tried before giving up.
const PORT_SEARCH_ATTEMPTS: u16 = 10;

/// Whether the artifacts listener is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkbenchServing {
    /// Serving on this port, beside the gateway.
    Running { port: u16 },
    /// Not serving; the reason is shown to the user.
    Unavailable { reason: String },
}

/// Start the artifacts listener for `dir` beside the gateway on `bind`. Returns
/// whether it is serving, the switch that stops it and its router (so another
/// transport can serve the same pages in-process) when it is. `api` is
/// where `/api` requests go once the hub router is bound to it.
///
/// Failing to bind is not fatal: Residuum runs without the workbench and the
/// web UI shows why (the reason is also logged at `error`). The listener runs
/// as a monitored task, so a panic or serve failure is logged too.
pub async fn start(
    bind: &str,
    gateway_port: u16,
    reserved: &[u16],
    dir: PathBuf,
    api: HubApi,
    pairing: crate::pairing::DevicePairing,
    sdk: SdkContext,
) -> (
    WorkbenchServing,
    Option<tokio::sync::watch::Sender<bool>>,
    Option<Router>,
) {
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
                None,
            );
        }
    };
    tracing::info!(addr = %format!("{bind}:{port}"), "workbench artifacts listening");

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let app = router(dir, api, pairing, gateway_port, sdk);
    let served = app.clone();
    residuum_core::util::spawn_monitored("workbench-listener", async move {
        if let Err(e) = axum::serve(listener, served)
            .with_graceful_shutdown(async move {
                shutdown_rx.wait_for(|stop| *stop).await.ok();
            })
            .await
        {
            tracing::error!(error = %e, "workbench artifacts listener failed");
        }
    });
    (
        WorkbenchServing::Running { port },
        Some(shutdown_tx),
        Some(app),
    )
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

/// Router for the artifacts listener, serving artifacts from `dir` and
/// forwarding `/api` to the hub router bound to `api`. `gateway_port` is the
/// web app's port on this machine, so a browser that lands here by mistake
/// can be sent to the app on the same host.
pub fn router(
    dir: PathBuf,
    api: HubApi,
    pairing: crate::pairing::DevicePairing,
    gateway_port: u16,
    sdk: SdkContext,
) -> Router {
    let ui = UiAddress::new(pairing.clone(), gateway_port);
    let state = ArtifactsState {
        dir,
        ui: ui.clone(),
        sdk,
    };
    let artifacts = Router::new()
        .route("/", get(home))
        .route(HANDOFF_PAGE_PATH, get(handoff_page))
        .route("/{name}", get(artifact_root))
        .route("/{name}/", get(artifact_index))
        .route("/{name}/{*rest}", get(artifact_file))
        .fallback(nothing_here)
        .with_state(state);
    forward::forwarding(artifacts, api, ui)
        .layer(axum::middleware::from_fn(
            crate::cross_site::reject_cross_site_requests,
        ))
        .layer(axum::middleware::from_fn_with_state(
            crate::pairing::GateState {
                pairing,
                surface: crate::pairing::Surface::Workbench,
            },
            crate::pairing::device_gate,
        ))
}

/// Stands in for the artifacts listener's router when the listener couldn't
/// start, so the workbench host still answers through Residuum Cloud.
pub fn unavailable_router(pairing: crate::pairing::DevicePairing) -> Router {
    Router::new().fallback(move |request: Parts| {
        let ui_origin = pairing.origin_of(crate::pairing::Surface::Ui);
        std::future::ready(unavailable(&request, ui_origin))
    })
}

/// A tab goes to the Workbench in the app, which shows why artifacts aren't
/// available; anything else is told here.
fn unavailable(request: &Parts, ui_origin: Option<String>) -> Response {
    if let Some(origin) = &ui_origin
        && browser_landing::is_top_level_navigation(&request.method, &request.headers)
    {
        return browser_landing::redirect_to_app_origin(origin, WORKBENCH_APP_PATH);
    }
    let link = ui_origin.map(|origin| format!("{origin}{WORKBENCH_APP_PATH}"));
    browser_landing::page_or_text(
        &request.method,
        &request.headers,
        StatusCode::SERVICE_UNAVAILABLE,
        &Landing::new(
            "Workbench unavailable",
            "Workbench artifacts aren't available on this Residuum instance right now. Check Residuum's logs for why its artifacts listener couldn't start.",
        )
        .with_app_link(link.as_deref(), "Open the Workbench"),
    )
}

#[derive(Clone)]
struct ArtifactsState {
    dir: PathBuf,
    ui: UiAddress,
    sdk: SdkContext,
}

/// Where the web app is, as seen from a browser on the artifacts host.
#[derive(Clone)]
pub(crate) struct UiAddress {
    pairing: crate::pairing::DevicePairing,
    gateway_port: u16,
}

impl UiAddress {
    /// The app as `pairing` knows it through Residuum Cloud, and on
    /// `gateway_port` locally.
    pub(crate) fn new(pairing: crate::pairing::DevicePairing, gateway_port: u16) -> Self {
        Self {
            pairing,
            gateway_port,
        }
    }

    /// The web app's origin for the browser that sent `request`: through
    /// Residuum Cloud the UI host Cloud announced, and locally the host the
    /// browser used, on the gateway's port. `None` when neither is known.
    pub(crate) fn origin_for(&self, request: &Parts) -> Option<String> {
        if crate::pairing::remote::remote_context(&request.extensions).is_some() {
            return self.pairing.origin_of(crate::pairing::Surface::Ui);
        }
        let host = request.headers.get(header::HOST)?.to_str().ok()?;
        let authority: axum::http::uri::Authority = host.parse().ok()?;
        Some(format!("http://{}:{}", authority.host(), self.gateway_port))
    }
}

/// The Workbench place in the app.
const WORKBENCH_APP_PATH: &str = "/team/workbench";

/// Answer a request that found nothing to serve. A tab goes to `app_target`
/// in the app when the app's address is known; a frame, or a browser with no
/// app to go to, gets the fallback page (linking there when it can); anything
/// else gets the words as text.
fn land(
    request: &Parts,
    ui: &UiAddress,
    status: StatusCode,
    app_target: &str,
    landing: Landing,
) -> Response {
    let origin = ui.origin_for(request);
    if browser_landing::is_top_level_navigation(&request.method, &request.headers)
        && let Some(origin) = &origin
    {
        return browser_landing::redirect_to_app_origin(origin, app_target);
    }
    let link = origin.map(|origin| format!("{origin}{app_target}"));
    let landing = landing.with_app_link(link.as_deref(), "Open the Workbench");
    browser_landing::page_or_text(&request.method, &request.headers, status, &landing)
}

/// The Workbench's address in the app, with a notice that there's no
/// artifact at the address the browser opened (named when the name could be
/// an artifact's).
fn missing_artifact_target(name: Option<&str>) -> String {
    let notice = ("notice", AppNotice::ArtifactMissing.code());
    match name.filter(|name| is_valid_artifact_name(name)) {
        Some(name) => browser_landing::app_path(WORKBENCH_APP_PATH, &[notice, ("artifact", name)]),
        None => browser_landing::app_path(WORKBENCH_APP_PATH, &[notice]),
    }
}

fn missing_artifact(request: &Parts, ui: &UiAddress, name: &str) -> Response {
    land(
        request,
        ui,
        StatusCode::NOT_FOUND,
        &missing_artifact_target(Some(name)),
        Landing::new(
            "No such artifact",
            format!(
                "There's no workbench artifact named \"{name}\". It may have been renamed or deleted."
            ),
        ),
    )
}

/// The page a paired browser lands on to bring its credential to this host.
///
/// The UI host mints a single-use token and sends the browser here with the
/// token and the page to open next in the URL fragment, which the browser
/// never sends anywhere. The script posts the token to this host, which
/// answers with this host's own device cookie, then opens the page. It is
/// served before the gate asks for a credential, since getting one is its
/// whole purpose.
async fn handoff_page(State(state): State<ArtifactsState>, request: Parts) -> Response {
    let actions = state
        .ui
        .origin_for(&request)
        .map_or_else(String::new, |origin| {
            format!(
                r#"<p class="actions" id="actions" hidden><a href="{}">Open the Workbench</a></p>"#,
                browser_landing::escape(&format!("{origin}{WORKBENCH_APP_PATH}"))
            )
        });
    let body = format!(
        r#"<h1 id="title">Opening the workbench</h1><p id="message" role="status">Opening the workbench in this browser…</p>{actions}"#
    );
    let mut resp = browser_landing::html_response(
        StatusCode::OK,
        "Opening the workbench",
        &body,
        Some(HANDOFF_SCRIPT),
    );
    resp.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

const HANDOFF_SCRIPT: &str = r#"
(function () {
  var title = document.getElementById("title");
  var message = document.getElementById("message");
  var actions = document.getElementById("actions");
  function fail(text) {
    title.textContent = "The workbench didn't open";
    message.textContent = text;
    if (actions) actions.hidden = false;
  }
  var params = new URLSearchParams(location.hash.slice(1));
  var token = params.get("token");
  var next = params.get("next") || "/";
  history.replaceState(null, "", location.pathname);
  if (!token) {
    fail("This link is missing its token. Open the workbench again from Residuum.");
    return;
  }
  if (!/^\/[A-Za-z0-9._~\/-]*$/.test(next) || next.indexOf("//") === 0) next = "/";
  fetch("/api/hub/pairing/handoff", {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token: token })
  }).then(function (response) {
    if (response.ok) { location.replace(next); return; }
    return response.json().then(function (body) {
      fail(body && body.error ? body.error : "The workbench couldn't be opened. Open it again from Residuum.");
    }, function () {
      fail("The workbench couldn't be opened. Open it again from Residuum.");
    });
  }, function () {
    fail("Couldn't reach Residuum. Check your connection, then open the workbench again.");
  });
})();
"#;

async fn home(State(state): State<ArtifactsState>, request: Parts) -> Response {
    land(
        &request,
        &state.ui,
        StatusCode::OK,
        WORKBENCH_APP_PATH,
        Landing::new(
            "Workbench artifacts",
            "Workbench artifacts open from the Workbench in Residuum.",
        ),
    )
}

async fn nothing_here(State(state): State<ArtifactsState>, request: Parts) -> Response {
    land(
        &request,
        &state.ui,
        StatusCode::NOT_FOUND,
        &missing_artifact_target(None),
        Landing::new(
            "Nothing here",
            "Nothing is served at this address. Workbench artifacts open from the Workbench in Residuum.",
        ),
    )
}

/// `/{artifact}` → `/{artifact}/`, so relative URLs in the artifact resolve inside it.
async fn artifact_root(
    State(state): State<ArtifactsState>,
    Path(name): Path<String>,
    request: Parts,
) -> Response {
    let exists = is_valid_artifact_name(&name)
        && discover_artifacts(&state.dir)
            .await
            .is_ok_and(|artifacts| artifacts.iter().any(|t| t.name == name));
    if !exists {
        return missing_artifact(&request, &state.ui, &name);
    }
    let query = request
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    Redirect::permanent(&format!("/{name}/{query}")).into_response()
}

async fn artifact_index(
    State(state): State<ArtifactsState>,
    Path(name): Path<String>,
    request: Parts,
) -> Response {
    serve(&state, &request, &name, "").await
}

async fn artifact_file(
    State(state): State<ArtifactsState>,
    Path((name, rest)): Path<(String, String)>,
    request: Parts,
) -> Response {
    serve(&state, &request, &name, &rest).await
}

async fn serve(state: &ArtifactsState, request: &Parts, name: &str, rest: &str) -> Response {
    if !is_valid_artifact_name(name) {
        return missing_artifact(request, &state.ui, name);
    }
    match read_artifact_file(&state.dir, name, rest, state.sdk).await {
        Ok(served) => {
            let body = match served.body {
                ArtifactBody::Bytes(bytes) => Body::from(bytes),
                // Streamed straight from disk so a large local artifact file
                // never has to be buffered whole in memory.
                ArtifactBody::File(file) => Body::from_stream(ReaderStream::new(file)),
            };
            let mut response = body.into_response();
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
        Err(ArtifactFileError::NoSuchArtifact(_)) => missing_artifact(request, &state.ui, name),
        Err(ArtifactFileError::NotFound { .. }) => land(
            request,
            &state.ui,
            StatusCode::NOT_FOUND,
            &browser_landing::app_path(
                &format!("{WORKBENCH_APP_PATH}/{name}"),
                &[("notice", AppNotice::ArtifactFileMissing.code())],
            ),
            Landing::new(
                "File not found",
                format!(
                    "The artifact \"{name}\" has no file \"{rest}\". It may have been moved or removed."
                ),
            ),
        ),
        Err(e @ ArtifactFileError::Io { .. }) => {
            tracing::error!(artifact = %name, path = %rest, error = %e, "failed to serve workbench artifact file");
            let link = state
                .ui
                .origin_for(request)
                .map(|origin| format!("{origin}{WORKBENCH_APP_PATH}"));
            browser_landing::page_or_text(
                &request.method,
                &request.headers,
                StatusCode::INTERNAL_SERVER_ERROR,
                &Landing::new(
                    "Couldn't open this artifact",
                    "Couldn't read this artifact's files. Try again, and if it keeps happening, Residuum's logs have the details.",
                )
                .with_app_link(link.as_deref(), "Open the Workbench"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    const SDK: SdkContext = SdkContext {
        version: "2026.09.23",
        features: &[],
    };

    /// Pairing state in a throwaway directory, so nothing remote is paired.
    fn test_pairing(dir: &std::path::Path) -> crate::pairing::DevicePairing {
        crate::pairing::DevicePairing::open(dir)
    }

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    async fn get_path(dir: &std::path::Path, path: &str) -> Response {
        router(
            dir.to_path_buf(),
            HubApi::new(),
            test_pairing(dir),
            7700,
            SDK,
        )
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
    }

    /// A hub stand-in: `/api/probe` says whether the request arrived marked,
    /// and any other path gets the page the real router's embedded web app
    /// would serve.
    fn stand_in_hub() -> Router {
        Router::new()
            .route(
                "/api/probe",
                axum::routing::any(|req: Request<Body>| async move {
                    if req.extensions().get::<forward::ArtifactsOrigin>().is_some() {
                        "marked"
                    } else {
                        "unmarked"
                    }
                }),
            )
            .fallback(|| async { "spa" })
    }

    /// The listener's router with the stand-in hub bound.
    fn listener(dir: &std::path::Path) -> Router {
        let api = HubApi::new();
        api.bind(stand_in_hub());
        router(dir.to_path_buf(), api, test_pairing(dir), 7700, SDK)
    }

    async fn send(app: Router, request: Request<Body>) -> (StatusCode, String) {
        let resp = app.oneshot(request).await.unwrap();
        let status = resp.status();
        (status, body_text(resp).await)
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
    async fn artifact_pages_carry_no_app_shell_policy() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("graph/index.html"),
            "<head></head><img src=https://cdn.example/a.png>",
        );
        write(&dir.path().join("graph/app.js"), "console.log(1)");
        write(&dir.path().join("chart.html"), "<head></head>");

        for path in ["/graph/", "/graph/app.js", "/chart/"] {
            let resp = get_path(dir.path(), path).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert!(
                resp.headers()
                    .get(header::CONTENT_SECURITY_POLICY)
                    .is_none(),
                "{path} must load what its author put in it"
            );
        }
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

        let post = router(
            dir.path().to_path_buf(),
            HubApi::new(),
            test_pairing(dir.path()),
            7700,
            SDK,
        )
        .oneshot(Request::post("/graph/").body(Body::empty()).unwrap())
        .await
        .unwrap();
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn api_requests_of_every_method_reach_the_hub_router_marked() {
        let dir = tempfile::tempdir().unwrap();
        for method in [
            axum::http::Method::GET,
            axum::http::Method::HEAD,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ] {
            let request = Request::builder()
                .method(method.clone())
                .uri("/api/probe")
                .body(Body::empty())
                .unwrap();
            let (status, body) = send(listener(dir.path()), request).await;
            assert_eq!(status, StatusCode::OK, "{method}");
            if method != axum::http::Method::HEAD {
                assert_eq!(body, "marked", "{method}");
            }
        }
    }

    #[tokio::test]
    async fn the_cross_site_guard_covers_the_api_and_lets_the_origin_call_itself() {
        let dir = tempfile::tempdir().unwrap();
        // The UI's own origin is the same site as the artifacts origin, so it
        // counts as another site here, like any other.
        for (site, allowed) in [
            ("cross-site", false),
            ("same-site", false),
            ("same-origin", true),
            ("none", true),
        ] {
            let write = Request::post("/api/probe")
                .header("sec-fetch-site", site)
                .body(Body::empty())
                .unwrap();
            let (write_status, _) = send(listener(dir.path()), write).await;
            assert_eq!(write_status == StatusCode::OK, allowed, "POST from {site}");

            let upgrade = Request::get("/api/probe")
                .header("sec-fetch-site", site)
                .header("upgrade", "websocket")
                .body(Body::empty())
                .unwrap();
            let (upgrade_status, _) = send(listener(dir.path()), upgrade).await;
            assert_eq!(
                upgrade_status == StatusCode::OK,
                allowed,
                "upgrade from {site}"
            );
        }
        let sandboxed = Request::post("/api/probe")
            .header("origin", "null")
            .body(Body::empty())
            .unwrap();
        let (sandboxed_status, _) = send(listener(dir.path()), sandboxed).await;
        assert_eq!(sandboxed_status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn nothing_but_the_api_reaches_the_hub_router() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "/",
            "/index.html",
            "/some/client/route",
            "/webhook/scout/deploy",
            "/cloud/callback",
            "/assets/app.js",
            "/apix/probe",
        ] {
            let (_, body) = send(
                listener(dir.path()),
                Request::get(path).body(Body::empty()).unwrap(),
            )
            .await;
            assert!(
                !matches!(body.as_str(), "spa" | "marked" | "unmarked"),
                "{path} reached the hub router"
            );
        }
    }

    #[tokio::test]
    async fn an_artifact_called_api_is_never_served_and_similar_names_are() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("api/index.html"), "<head></head>artifact");
        write(&dir.path().join("api-tools.html"), "<head></head>tools");
        write(&dir.path().join("my-api/index.html"), "<head></head>mine");

        for path in ["/api", "/api/", "/api/index.html"] {
            let (_, body) = send(
                listener(dir.path()),
                Request::get(path).body(Body::empty()).unwrap(),
            )
            .await;
            assert_eq!(body, "spa", "{path} is the hub's, not the artifact's");
        }
        for (path, content) in [("/api-tools/", "tools"), ("/my-api/", "mine")] {
            let (status, body) = send(
                listener(dir.path()),
                Request::get(path).body(Body::empty()).unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.contains(content), "{path}");
        }
    }

    /// A request as a browser on this machine sends it to `localhost:7702`:
    /// opening a tab, loading a frame, or fetching from a script.
    fn browser_request(path: &str, dest: &str) -> Request<Body> {
        let mut builder = Request::get(path)
            .header(header::HOST, "localhost:7702")
            .header("sec-fetch-dest", dest);
        if dest == "empty" {
            builder = builder.header(header::ACCEPT, "*/*");
        } else {
            builder = builder
                .header("sec-fetch-mode", "navigate")
                .header(header::ACCEPT, "text/html,*/*;q=0.8");
        }
        builder.body(Body::empty()).unwrap()
    }

    async fn location_of(dir: &std::path::Path, request: Request<Body>) -> String {
        let resp = listener(dir).oneshot(request).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "redirected");
        resp.headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn a_tab_that_finds_nothing_goes_to_the_workbench_on_the_same_host() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("graph/index.html"), "<head></head>");
        for (path, expected) in [
            ("/", "http://localhost:7700/team/workbench"),
            (
                "/gone/",
                "http://localhost:7700/team/workbench?notice=artifact-missing&artifact=gone",
            ),
            (
                "/gone",
                "http://localhost:7700/team/workbench?notice=artifact-missing&artifact=gone",
            ),
            (
                "/Bad.Name/",
                "http://localhost:7700/team/workbench?notice=artifact-missing",
            ),
            (
                "/graph/missing.js",
                "http://localhost:7700/team/workbench/graph?notice=artifact-file-missing",
            ),
        ] {
            assert_eq!(
                location_of(dir.path(), browser_request(path, "document")).await,
                expected,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn a_frame_gets_the_fallback_page_and_a_script_gets_text() {
        let dir = tempfile::tempdir().unwrap();

        let frame = listener(dir.path())
            .oneshot(browser_request("/gone/", "iframe"))
            .await
            .unwrap();
        assert_eq!(frame.status(), StatusCode::NOT_FOUND, "a frame: not found");
        let page = body_text(frame).await;
        assert!(page.contains("<h1>No such artifact</h1>"), "{page}");
        assert!(
            page.contains(r#"href="http://localhost:7700/team/workbench?notice=artifact-missing&amp;artifact=gone""#),
            "links to the Workbench: {page}"
        );

        let fetch = listener(dir.path())
            .oneshot(browser_request("/gone/app.js", "empty"))
            .await
            .unwrap();
        assert_eq!(fetch.status(), StatusCode::NOT_FOUND, "a script: not found");
        assert_eq!(
            fetch.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8",
            "as text"
        );
        assert!(
            body_text(fetch)
                .await
                .contains("no workbench artifact named \"gone\""),
            "saying why"
        );
    }

    #[tokio::test]
    async fn without_a_host_a_tab_gets_the_fallback_page() {
        let dir = tempfile::tempdir().unwrap();
        let request = Request::get("/")
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-mode", "navigate")
            .body(Body::empty())
            .unwrap();
        let resp = listener(dir.path()).oneshot(request).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "the listener's own page");
        let page = body_text(resp).await;
        assert!(
            page.contains("Workbench artifacts open from the Workbench in Residuum."),
            "{page}"
        );
        assert!(!page.contains("<a "), "no link to an unknown app: {page}");
    }

    #[tokio::test]
    async fn the_handoff_page_is_the_fallback_page_with_its_script() {
        let dir = tempfile::tempdir().unwrap();
        let resp = get_path(dir.path(), HANDOFF_PAGE_PATH).await;
        assert_eq!(resp.status(), StatusCode::OK, "served");
        assert_eq!(
            resp.headers().get(header::REFERRER_POLICY).unwrap(),
            "no-referrer",
            "the token never leaves in a referrer"
        );
        let page = body_text(resp).await;
        assert!(page.contains("--color-stone-0"), "styled from the tokens");
        assert!(
            page.contains("/api/hub/pairing/handoff"),
            "carries the script"
        );
    }

    #[tokio::test]
    async fn an_app_redirect_from_the_api_is_pointed_at_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let api = HubApi::new();
        api.bind(Router::new().route(
            "/api/agents/scout/files/x",
            get(|| async {
                browser_landing::app_redirect(
                    "/agent/scout?notice=file-link-expired",
                    browser_landing::Fallback {
                        title: "Expired",
                        message: "This file link has expired.",
                    },
                )
            }),
        ));
        let app = router(
            dir.path().to_path_buf(),
            api,
            test_pairing(dir.path()),
            7700,
            SDK,
        );
        let resp = app
            .clone()
            .oneshot(browser_request("/api/agents/scout/files/x", "document"))
            .await
            .unwrap();
        assert_eq!(
            resp.headers().get(header::LOCATION).unwrap(),
            "http://localhost:7700/agent/scout?notice=file-link-expired",
            "to the app's origin"
        );

        let no_host = Request::get("/api/agents/scout/files/x")
            .header("sec-fetch-dest", "document")
            .body(Body::empty())
            .unwrap();
        let fallback = app.oneshot(no_host).await.unwrap();
        assert_eq!(
            fallback.status(),
            StatusCode::NOT_FOUND,
            "nowhere to send it"
        );
        assert!(
            body_text(fallback)
                .await
                .contains("This file link has expired."),
            "so the fallback says why"
        );
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
