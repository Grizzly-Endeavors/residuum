//! File serving for WebSocket-connected clients.
//!
//! Registers files with a TTL and serves them via HTTP. WebSocket messages
//! reference files by URL rather than embedding binary data.
//!
//! A file inside the workspace is served by its workspace-relative path
//! instead (see [`FileRegistry::url_for`]) — a link that keeps working for
//! as long as the file exists, not just for the TTL. Only a file outside
//! the workspace (or the workspace boundary was never configured) falls
//! back to the token-registry scheme below.
//!
//! Every response carrying a file an agent wrote goes through
//! [`mark_agent_file`], which keeps the browser from running it with the
//! hub's origin.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::sync::RwLock;
use tokio::time::Instant;

use crate::gateway::browser_landing::{self, AppNotice, Fallback};

/// The `Content-Security-Policy` of a file an agent wrote, served from the
/// hub's own origin. An agent can be steered by what it read into writing an
/// HTML or SVG file, and a link to it in a reply opens it as a page; on the
/// hub's origin its scripts could call the whole API as the user. `sandbox`
/// gives the page an opaque origin of its own instead, so it still opens and
/// runs but reaches nothing of the hub's, and the allowances keep what a page
/// a person opens on purpose needs: its scripts, links that open a tab, and
/// downloads.
const AGENT_FILE_CSP: &str = "sandbox allow-scripts allow-popups allow-downloads";

/// Mark a response that carries a file an agent wrote: `nosniff`, so the
/// browser reads it only as the type it is sent with, and, for every type but
/// a PDF, [`AGENT_FILE_CSP`]. Chromium's PDF viewer refuses to show a PDF in a
/// sandboxed document, and a PDF's own scripts never run with the hub's
/// origin, so a PDF is left to open as it always has.
pub(crate) fn mark_agent_file(headers: &mut HeaderMap, mime_type: &str) {
    use axum::http::{HeaderValue, header};

    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    let essence = mime_type.split(';').next().unwrap_or_default().trim();
    if !essence.eq_ignore_ascii_case("application/pdf") {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(AGENT_FILE_CSP),
        );
    }
}

/// How long a registered file remains available (1 hour).
const FILE_TTL_SECS: u64 = 3600;

/// How often the cleanup task sweeps expired entries (10 minutes).
const CLEANUP_INTERVAL_SECS: u64 = 600;

/// A registered file entry with expiration.
struct FileEntry {
    path: PathBuf,
    mime_type: String,
    filename: String,
    expires_at: Instant,
}

/// Thread-safe registry of files available for HTTP serving.
#[derive(Clone)]
pub struct FileRegistry {
    entries: Arc<RwLock<HashMap<String, FileEntry>>>,
    /// The workspace root, when configured — see [`Self::with_workspace_root`].
    workspace_root: Option<PathBuf>,
    /// The agent whose files this registry serves. Every URL it returns is
    /// under `/api/agents/{agent}/`, so a client can use it as given.
    agent_name: String,
}

impl FileRegistry {
    /// Create a new empty file registry for the agent named `agent_name`,
    /// with no workspace root configured (every file is served through the
    /// expiring token scheme).
    #[must_use]
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            workspace_root: None,
            agent_name: agent_name.into(),
        }
    }

    /// Serve a file under `root` by its durable workspace-relative URL
    /// (see [`Self::url_for`]) instead of the expiring token scheme.
    #[must_use]
    pub fn with_workspace_root(mut self, root: PathBuf) -> Self {
        self.workspace_root = Some(root);
        self
    }

    /// Register a file and return its unique serving ID.
    pub async fn register(&self, path: PathBuf, mime_type: String, filename: String) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let entry = FileEntry {
            path,
            mime_type,
            filename,
            expires_at: Instant::now() + std::time::Duration::from_secs(FILE_TTL_SECS),
        };
        self.entries.write().await.insert(id.clone(), entry);
        id
    }

    /// The URL a client should use to fetch `path` (an absolute filesystem
    /// path already known to exist and be readable): a durable
    /// workspace-relative URL when `path` is inside the configured
    /// workspace root and not blocked from exposure by
    /// `workspace::access::is_blocked_path`, else the existing
    /// [`Self::register`]-backed token URL.
    pub async fn url_for(&self, path: PathBuf, mime_type: String, filename: String) -> String {
        if let Some(relative) = self.workspace_relative_path(&path).await {
            let encoded: String =
                url::form_urlencoded::byte_serialize(relative.as_bytes()).collect();
            return format!("{}/files/workspace?path={encoded}", self.url_prefix());
        }
        let id = self.register(path, mime_type, filename).await;
        format!("{}/files/{id}", self.url_prefix())
    }

    /// The hub URL prefix of this agent's routes.
    fn url_prefix(&self) -> String {
        format!("/api/agents/{}", self.agent_name)
    }

    /// `path`'s location relative to the workspace root, as a `/`-separated
    /// string, when it's inside the workspace and not one of the paths
    /// `workspace::access::is_blocked_path` hides — `None` otherwise
    /// (outside the workspace, no workspace root configured, or either
    /// path fails to canonicalize).
    async fn workspace_relative_path(&self, path: &Path) -> Option<String> {
        let root = self.workspace_root.as_ref()?;
        let canonical_root = tokio::fs::canonicalize(root).await.ok()?;
        let canonical_path = tokio::fs::canonicalize(path).await.ok()?;
        let relative = canonical_path.strip_prefix(&canonical_root).ok()?;
        let relative_str = relative.to_str()?.replace(std::path::MAIN_SEPARATOR, "/");
        if crate::workspace::access::is_blocked_path(&relative_str) {
            return None;
        }
        Some(relative_str)
    }

    /// Look up a file by ID. Returns `(path, mime_type, filename)` if found and not expired.
    pub async fn get(&self, id: &str) -> Option<(PathBuf, String, String)> {
        let entries = self.entries.read().await;
        let entry = entries.get(id)?;
        if entry.expires_at < Instant::now() {
            return None;
        }
        Some((
            entry.path.clone(),
            entry.mime_type.clone(),
            entry.filename.clone(),
        ))
    }

    /// Remove all expired entries. Returns `(removed, remaining)`.
    pub async fn sweep_expired(&self) -> (usize, usize) {
        sweep_entries(&self.entries).await
    }

    /// Spawn a background task that periodically sweeps expired entries. It
    /// holds the entries weakly, so it ends on its own once every handle to
    /// the registry is gone (the agent that owns it stopped).
    pub fn spawn_cleanup_task(&self) {
        let entries = Arc::downgrade(&self.entries);
        crate::util::spawn_in_span(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(CLEANUP_INTERVAL_SECS));
            loop {
                interval.tick().await;
                let Some(entries) = entries.upgrade() else {
                    return;
                };
                let (removed, remaining) = sweep_entries(&entries).await;
                if removed > 0 {
                    tracing::debug!(removed, remaining, "swept expired file entries");
                }
            }
        });
    }
}

/// Remove the expired entries from `entries`. Returns `(removed, remaining)`.
async fn sweep_entries(entries: &RwLock<HashMap<String, FileEntry>>) -> (usize, usize) {
    let now = Instant::now();
    let mut entries = entries.write().await;
    let before = entries.len();
    entries.retain(|_, entry| entry.expires_at > now);
    let remaining = entries.len();
    (before - remaining, remaining)
}

/// Axum handler to serve a registered file by ID.
///
/// A link whose registration expired (or whose file is gone) sends a browser
/// that opened it to the agent's chat, which says the link expired and how to
/// get the file again; anything else gets a JSON error.
pub async fn serve_file(
    method: Method,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::State(registry): axum::extract::State<FileRegistry>,
) -> Response {
    let Some((path, mime_type, filename)) = registry.get(&id).await else {
        tracing::debug!(file_id = %id, "file link expired or unknown, refusing to serve");
        return expired_link(&registry, &method, &headers);
    };

    let Ok(bytes) = tokio::fs::read(&path).await else {
        tracing::warn!(file_id = %id, path = %path.display(), "registered file not readable");
        return expired_link(&registry, &method, &headers);
    };

    respond_with_file(bytes, &mime_type, &filename)
}

/// The answer for a token link that no longer leads to a file.
fn expired_link(registry: &FileRegistry, method: &Method, headers: &HeaderMap) -> Response {
    if browser_landing::is_top_level_navigation(method, headers) {
        return browser_landing::app_redirect(
            &browser_landing::app_path(
                &format!("/agent/{}", registry.agent_name),
                &[("notice", AppNotice::FileLinkExpired.code())],
            ),
            Fallback {
                title: "This link has expired",
                message: "This file link has expired. Ask the agent to send the file again.",
            },
        );
    }
    json_error(
        StatusCode::NOT_FOUND,
        "This file link has expired. Ask the agent to send the file again.",
    )
}

/// Query parameters for [`serve_workspace_file`].
#[derive(Debug, serde::Deserialize)]
pub struct WorkspaceFileQuery {
    /// The file's path relative to the workspace root.
    path: String,
}

/// Axum handler serving a file by its workspace-relative path — the
/// counterpart to [`serve_file`] for files inside the workspace, which
/// never expires as long as the file itself still exists there.
///
/// When the file can't be served, a browser that opened the link goes to the
/// file in the agent's Files, which shows what became of it; anything else
/// gets a JSON error.
pub async fn serve_workspace_file(
    method: Method,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<WorkspaceFileQuery>,
    axum::extract::State(registry): axum::extract::State<FileRegistry>,
) -> Response {
    match read_workspace_file(&registry, &query.path).await {
        Ok(response) => response,
        Err((status, message)) => {
            if browser_landing::is_top_level_navigation(&method, &headers) {
                let panel = format!("file:{}", query.path);
                return browser_landing::app_redirect(
                    &browser_landing::app_path(
                        &format!("/agent/{}/files", registry.agent_name),
                        &[("panel", &panel)],
                    ),
                    Fallback {
                        title: "File unavailable",
                        message: "This file isn't available from the workspace. It may have been moved or deleted.",
                    },
                );
            }
            json_error(status, message)
        }
    }
}

/// The response serving the workspace file at `relative`, or the status and
/// plain-language reason it can't be served.
async fn read_workspace_file(
    registry: &FileRegistry,
    relative: &str,
) -> Result<Response, (StatusCode, &'static str)> {
    const MISSING: (StatusCode, &str) = (
        StatusCode::NOT_FOUND,
        "This file is no longer in the workspace. It may have been moved or deleted.",
    );
    let Some(root) = &registry.workspace_root else {
        tracing::warn!("workspace file link requested but no workspace root is configured");
        return Err(MISSING);
    };

    if crate::workspace::access::is_blocked_path(relative) {
        tracing::warn!(path = %relative, "refusing to serve a blocked workspace path");
        return Err((
            StatusCode::NOT_FOUND,
            "This file can't be opened from a link.",
        ));
    }

    let Ok(canonical_root) = tokio::fs::canonicalize(root).await else {
        tracing::warn!(root = %root.display(), "failed to canonicalize workspace root while serving a file link");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't read the workspace. Residuum's logs have the details.",
        ));
    };

    let target = root.join(relative);
    let canonical_target = match tokio::fs::canonicalize(&target).await {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(path = %relative, error = %e, "workspace file link points at a file that no longer exists");
            return Err(MISSING);
        }
    };
    if !canonical_target.starts_with(&canonical_root) {
        tracing::warn!(path = %relative, "workspace file link escaped the workspace root, refusing");
        return Err((
            StatusCode::FORBIDDEN,
            "This link points outside the workspace.",
        ));
    }

    let Ok(bytes) = tokio::fs::read(&canonical_target).await else {
        tracing::warn!(path = %relative, "workspace file link target not readable");
        return Err(MISSING);
    };

    let filename = canonical_target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let mime_type = crate::interfaces::attachment::detect_mime_type(&canonical_target);
    Ok(respond_with_file(bytes, &mime_type, filename))
}

/// A `{ "error": message }` response, the API's error shape.
fn json_error(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
}

/// Build the HTTP response for a file's bytes, with a content-disposition
/// naming `filename` (quotes and backslashes stripped, since they'd need
/// escaping in the header value), when it parses as a valid header value,
/// `mime_type` as the content type, and the headers of [`mark_agent_file`].
pub(crate) fn respond_with_file(bytes: Vec<u8>, mime_type: &str, filename: &str) -> Response {
    use axum::http::{HeaderValue, header};

    let safe_filename: String = filename
        .chars()
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    let disposition = format!("inline; filename=\"{safe_filename}\"");
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(mime_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    mark_agent_file(headers, mime_type);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_lookup() {
        let registry = FileRegistry::new("scout");
        let id = registry
            .register(
                PathBuf::from("/tmp/test.pdf"),
                "application/pdf".to_string(),
                "test.pdf".to_string(),
            )
            .await;

        let entry = registry.get(&id).await;
        assert!(entry.is_some(), "registered file should be found");
        let (path, mime, filename) = entry.unwrap();
        assert_eq!(path, PathBuf::from("/tmp/test.pdf"));
        assert_eq!(mime, "application/pdf");
        assert_eq!(filename, "test.pdf");
    }

    #[tokio::test]
    async fn lookup_unknown_id_returns_none() {
        let registry = FileRegistry::new("scout");
        assert!(registry.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn sweep_removes_expired() {
        let registry = FileRegistry::new("scout");
        // Insert with already-expired time
        {
            let mut entries = registry.entries.write().await;
            entries.insert(
                "expired-id".to_string(),
                FileEntry {
                    path: PathBuf::from("/tmp/old.pdf"),
                    mime_type: "application/pdf".to_string(),
                    filename: "old.pdf".to_string(),
                    expires_at: Instant::now() - std::time::Duration::from_secs(1),
                },
            );
        }
        let (removed, remaining) = registry.sweep_expired().await;
        assert_eq!(removed, 1, "one expired entry should be removed");
        assert_eq!(remaining, 0, "no entries should remain");
        assert!(registry.get("expired-id").await.is_none());
    }

    #[tokio::test]
    async fn sweep_concurrent_register_does_not_underflow() {
        // Regression: earlier implementation computed removed = before - after across
        // separate lock acquisitions, which could underflow if a register raced in.
        let registry = FileRegistry::new("scout");
        registry
            .register(
                PathBuf::from("/tmp/live.pdf"),
                "application/pdf".to_string(),
                "live.pdf".to_string(),
            )
            .await;
        let (removed, remaining) = registry.sweep_expired().await;
        assert_eq!(removed, 0, "no expired entries should be removed");
        assert_eq!(remaining, 1, "live entry should remain");
    }

    async fn body_text(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn url_for_a_file_inside_the_workspace_is_a_durable_workspace_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        let file_path = dir.path().join("notes/plan.md");
        std::fs::write(&file_path, "hello").unwrap();

        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().to_path_buf());
        let url = registry
            .url_for(
                file_path,
                "text/markdown".to_string(),
                "plan.md".to_string(),
            )
            .await;

        assert_eq!(
            url, "/api/agents/scout/files/workspace?path=notes%2Fplan.md",
            "should be a workspace-path link, not an expiring token: {url}"
        );
    }

    #[tokio::test]
    async fn url_for_a_file_outside_the_workspace_falls_back_to_a_token_link() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file_path = outside.path().join("scratch.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().to_path_buf());
        let url = registry
            .url_for(
                file_path,
                "text/plain".to_string(),
                "scratch.txt".to_string(),
            )
            .await;

        assert!(
            url.starts_with("/api/agents/scout/files/")
                && !url.starts_with("/api/agents/scout/files/workspace"),
            "a file outside the workspace should still get a token link: {url}"
        );
    }

    #[tokio::test]
    async fn url_for_with_no_workspace_root_configured_always_uses_a_token_link() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("scratch.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let registry = FileRegistry::new("scout");
        let url = registry
            .url_for(
                file_path,
                "text/plain".to_string(),
                "scratch.txt".to_string(),
            )
            .await;

        assert!(
            url.starts_with("/api/agents/scout/files/")
                && !url.starts_with("/api/agents/scout/files/workspace"),
            "got: {url}"
        );
    }

    /// What a script's `fetch` sends.
    fn fetch_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "*/*".parse().unwrap());
        headers.insert("sec-fetch-dest", "empty".parse().unwrap());
        headers
    }

    /// What a browser opening a link in a tab sends.
    fn tab_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("accept", "text/html,*/*;q=0.8".parse().unwrap());
        headers.insert("sec-fetch-dest", "document".parse().unwrap());
        headers.insert("sec-fetch-mode", "navigate".parse().unwrap());
        headers
    }

    async fn workspace_file(registry: FileRegistry, path: &str, headers: HeaderMap) -> Response {
        serve_workspace_file(
            Method::GET,
            headers,
            axum::extract::Query(WorkspaceFileQuery {
                path: path.to_string(),
            }),
            axum::extract::State(registry),
        )
        .await
    }

    async fn token_file(registry: FileRegistry, id: &str, headers: HeaderMap) -> Response {
        serve_file(
            Method::GET,
            headers,
            axum::extract::Path(id.to_string()),
            axum::extract::State(registry),
        )
        .await
    }

    fn location(resp: &Response) -> &str {
        resp.headers()
            .get(axum::http::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
    }

    #[tokio::test]
    async fn serve_workspace_file_serves_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.md"), "# hello").unwrap();
        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().to_path_buf());

        for headers in [fetch_headers(), tab_headers()] {
            let resp = workspace_file(registry.clone(), "report.md", headers).await;
            assert_eq!(resp.status(), StatusCode::OK, "served");
            assert_eq!(body_text(resp).await, "# hello", "the file");
        }
    }

    fn header_of(resp: &Response, name: axum::http::HeaderName) -> Option<&str> {
        resp.headers().get(name).map(|v| v.to_str().unwrap())
    }

    #[tokio::test]
    async fn an_agents_page_opens_sandboxed_away_from_the_hubs_origin() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        for name in ["page.html", "drawing.svg", "photo.png"] {
            std::fs::write(
                dir.path().join(name),
                "<script>fetch('/api/hub/secrets')</script>",
            )
            .unwrap();
            std::fs::write(outside.path().join(name), "x").unwrap();
        }
        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().to_path_buf());

        for name in ["page.html", "drawing.svg", "photo.png"] {
            let id = registry
                .register(
                    outside.path().join(name),
                    crate::interfaces::attachment::detect_mime_type(&outside.path().join(name)),
                    name.to_string(),
                )
                .await;
            for resp in [
                workspace_file(registry.clone(), name, tab_headers()).await,
                token_file(registry.clone(), &id, tab_headers()).await,
            ] {
                assert_eq!(resp.status(), StatusCode::OK, "{name}");
                assert_eq!(
                    header_of(&resp, axum::http::header::CONTENT_SECURITY_POLICY),
                    Some("sandbox allow-scripts allow-popups allow-downloads"),
                    "{name}"
                );
                assert_eq!(
                    header_of(&resp, axum::http::header::X_CONTENT_TYPE_OPTIONS),
                    Some("nosniff"),
                    "{name}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_pdf_is_not_sandboxed_so_the_browsers_viewer_shows_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.pdf"), "%PDF-1.7").unwrap();
        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().to_path_buf());

        let resp = workspace_file(registry, "report.pdf", tab_headers()).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            header_of(&resp, axum::http::header::CONTENT_TYPE),
            Some("application/pdf")
        );
        assert!(
            resp.headers()
                .get(axum::http::header::CONTENT_SECURITY_POLICY)
                .is_none()
        );
        assert_eq!(
            header_of(&resp, axum::http::header::X_CONTENT_TYPE_OPTIONS),
            Some("nosniff")
        );
    }

    #[tokio::test]
    async fn serve_workspace_file_missing_file_explains_itself_to_a_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().to_path_buf());

        let resp = workspace_file(registry, "gone.md", fetch_headers()).await;

        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "missing");
        let body: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert!(
            body.get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|e| e.contains("no longer in the workspace")),
            "a missing file explains itself in the API's error shape: {body}"
        );
    }

    #[tokio::test]
    async fn a_tab_opening_a_workspace_file_that_cant_be_served_goes_to_it_in_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ws/.index")).unwrap();
        std::fs::write(dir.path().join("ws/.index/segment.bin"), "data").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "nope").unwrap();
        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().join("ws"));

        for (path, expected) in [
            (
                "notes/gone.md",
                "/agent/scout/files?panel=file%3Anotes%2Fgone.md",
            ),
            (
                ".index/segment.bin",
                "/agent/scout/files?panel=file%3A.index%2Fsegment.bin",
            ),
            (
                "../secret.txt",
                "/agent/scout/files?panel=file%3A..%2Fsecret.txt",
            ),
        ] {
            let resp = workspace_file(registry.clone(), path, tab_headers()).await;
            assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{path}");
            assert_eq!(location(&resp), expected, "{path}");
            assert!(
                resp.extensions()
                    .get::<browser_landing::AppRedirect>()
                    .is_some(),
                "{path}: marked as a redirect into the app"
            );
        }
    }

    #[tokio::test]
    async fn serve_workspace_file_rejects_a_path_that_escapes_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ws")).unwrap();
        let workspace = dir.path().join("ws");
        std::fs::write(dir.path().join("secret.txt"), "nope").unwrap();
        let registry = FileRegistry::new("scout").with_workspace_root(workspace);

        let resp = workspace_file(registry, "../secret.txt", fetch_headers()).await;

        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "refused");
        assert!(
            !body_text(resp).await.contains("nope"),
            "the file isn't served"
        );
    }

    #[tokio::test]
    async fn serve_workspace_file_rejects_a_blocked_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".index")).unwrap();
        std::fs::write(dir.path().join(".index/segment.bin"), "data").unwrap();
        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().to_path_buf());

        let resp = workspace_file(registry, ".index/segment.bin", fetch_headers()).await;

        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "refused");
    }

    #[tokio::test]
    async fn an_expired_token_link_sends_a_tab_to_the_chat_and_tells_a_fetch() {
        let registry = FileRegistry::new("scout");

        let tab = token_file(registry.clone(), "expired-id", tab_headers()).await;
        assert_eq!(tab.status(), StatusCode::SEE_OTHER, "a tab is redirected");
        assert_eq!(
            location(&tab),
            "/agent/scout?notice=file-link-expired",
            "to the agent's chat, with the notice"
        );

        let fetch = token_file(registry, "expired-id", fetch_headers()).await;
        assert_eq!(fetch.status(), StatusCode::NOT_FOUND, "a fetch gets 404");
        let body: serde_json::Value = serde_json::from_str(&body_text(fetch).await).unwrap();
        assert!(
            body.get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|e| e.contains("has expired")),
            "with the reason: {body}"
        );
    }

    #[tokio::test]
    async fn a_token_link_whose_file_is_gone_counts_as_expired() {
        let dir = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new("scout");
        let id = registry
            .register(
                dir.path().join("removed.pdf"),
                "application/pdf".to_string(),
                "removed.pdf".to_string(),
            )
            .await;

        let tab = token_file(registry, &id, tab_headers()).await;
        assert_eq!(
            location(&tab),
            "/agent/scout?notice=file-link-expired",
            "to the agent's chat, with the notice"
        );
    }
}
