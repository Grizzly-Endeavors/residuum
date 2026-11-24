use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, OnceCell, RwLock};

use residuum_config::WorkspaceLayout;
use residuum_net::browser_landing::{self, AppNotice, Fallback};

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
/// sandboxed document, so a PDF opens unsandboxed and relies on the browser's
/// PDF viewer to keep the file's own scripts away from the hub's origin.
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

const FILE_TTL_SECS: i64 = 3600;

const CLEANUP_INTERVAL_SECS: u64 = 600;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct FileEntry {
    path: PathBuf,
    mime_type: String,
    filename: String,
    expires_at: DateTime<Utc>,
}

impl FileEntry {
    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now || self.expires_at > now + chrono::Duration::seconds(FILE_TTL_SECS)
    }
}

type Entries = HashMap<String, FileEntry>;

struct TokenLinks {
    entries: OnceCell<RwLock<Entries>>,
    store: Option<PathBuf>,
    persist_lock: Mutex<()>,
}

impl TokenLinks {
    fn new(store: Option<PathBuf>) -> Self {
        Self {
            entries: OnceCell::new(),
            store,
            persist_lock: Mutex::new(()),
        }
    }

    async fn entries(&self) -> &RwLock<Entries> {
        self.entries
            .get_or_init(|| async {
                let entries = match &self.store {
                    Some(store) => load_file_links(store).await,
                    None => HashMap::new(),
                };
                RwLock::new(entries)
            })
            .await
    }

    async fn insert(&self, id: String, entry: FileEntry) {
        let _persist = self.persist_lock.lock().await;
        let snapshot = {
            let mut entries = self.entries().await.write().await;
            entries.insert(id, entry);
            self.store.as_ref().map(|_| entries.clone())
        };
        if let Some(snapshot) = snapshot {
            self.save(&snapshot).await;
        }
    }

    async fn find_active(&self, path: &Path) -> Option<String> {
        let now = Utc::now();
        let entries = self.entries().await.read().await;
        entries
            .iter()
            .filter(|(_, entry)| entry.path == path && !entry.is_expired(now))
            .max_by(|(id_a, a), (id_b, b)| {
                a.expires_at.cmp(&b.expires_at).then_with(|| id_a.cmp(id_b))
            })
            .map(|(id, _)| id.clone())
    }

    async fn sweep_expired(&self) -> (usize, usize) {
        let _persist = self.persist_lock.lock().await;
        let now = Utc::now();
        let (removed, remaining, snapshot) = {
            let mut entries = self.entries().await.write().await;
            let before = entries.len();
            entries.retain(|_, entry| !entry.is_expired(now));
            let remaining = entries.len();
            let removed = before - remaining;
            let snapshot = (removed > 0 && self.store.is_some()).then(|| entries.clone());
            (removed, remaining, snapshot)
        };
        if let Some(snapshot) = snapshot {
            self.save(&snapshot).await;
        }
        (removed, remaining)
    }

    async fn save(&self, entries: &Entries) {
        let Some(store) = &self.store else {
            return;
        };
        if let Err(e) = save_file_links(store, entries).await {
            tracing::warn!(
                path = %store.display(),
                error = %format!("{e:#}"),
                "couldn't save the agent's file links; links to files outside the workspace stop working when the agent stops"
            );
        }
    }
}

async fn load_file_links(path: &Path) -> Entries {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "couldn't read the agent's saved file links; links to files outside the workspace sent earlier won't open"
            );
            return HashMap::new();
        }
    };
    if contents.trim().is_empty() {
        return HashMap::new();
    }
    match serde_json::from_str(&contents) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "couldn't parse the agent's saved file links; links to files outside the workspace sent earlier won't open"
            );
            HashMap::new()
        }
    }
}

async fn save_file_links(path: &Path, entries: &Entries) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(entries).context("failed to serialize file links")?;
    residuum_core::util::fs::atomic_write(path, json)
        .await
        .with_context(|| format!("failed to write file links to {}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SentAttachment {
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

#[derive(Clone)]
pub struct FileRegistry {
    links: Arc<TokenLinks>,
    workspace_root: Option<PathBuf>,
    agent_name: String,
}

impl FileRegistry {
    #[must_use]
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            links: Arc::new(TokenLinks::new(None)),
            workspace_root: None,
            agent_name: agent_name.into(),
        }
    }

    #[must_use]
    pub fn for_agent(agent_name: impl Into<String>, layout: &WorkspaceLayout) -> Self {
        Self {
            links: Arc::new(TokenLinks::new(Some(layout.file_links_json()))),
            workspace_root: Some(layout.root().to_path_buf()),
            agent_name: agent_name.into(),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_workspace_root(mut self, root: PathBuf) -> Self {
        self.workspace_root = Some(root);
        self
    }

    pub async fn register(&self, path: PathBuf, mime_type: String, filename: String) -> String {
        let expires_at = Utc::now() + chrono::Duration::seconds(FILE_TTL_SECS);
        self.register_until(path, mime_type, filename, expires_at)
            .await
    }

    async fn register_until(
        &self,
        path: PathBuf,
        mime_type: String,
        filename: String,
        expires_at: DateTime<Utc>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let entry = FileEntry {
            path,
            mime_type,
            filename,
            expires_at,
        };
        self.links.insert(id.clone(), entry).await;
        id
    }

    pub async fn url_for(&self, path: PathBuf, mime_type: String, filename: String) -> String {
        if let Some(relative) = self.workspace_relative_path(&path).await {
            let encoded: String =
                url::form_urlencoded::byte_serialize(relative.as_bytes()).collect();
            return format!("{}/files/workspace?path={encoded}", self.url_prefix());
        }
        let id = match self.links.find_active(&path).await {
            Some(existing) => existing,
            None => self.register(path, mime_type, filename).await,
        };
        format!("{}/files/{id}", self.url_prefix())
    }

    pub async fn resolve_sent_attachment(
        &self,
        file_path: &Path,
        caption: Option<String>,
    ) -> Option<SentAttachment> {
        let resolved_path = if file_path.is_relative()
            && let Some(root) = &self.workspace_root
        {
            root.join(file_path)
        } else {
            file_path.to_path_buf()
        };
        let att = match residuum_bus::attachment::FileAttachment::from_path(&resolved_path).await {
            Ok(att) => att,
            Err(err) => {
                tracing::debug!(
                    path = %resolved_path.display(),
                    error = %err,
                    "skipping sent attachment: file is no longer readable on disk",
                );
                return None;
            }
        };
        let url = self
            .url_for(att.path, att.mime_type.clone(), att.filename.clone())
            .await;
        Some(SentAttachment {
            filename: att.filename,
            mime_type: att.mime_type,
            size: att.size,
            url,
            caption,
        })
    }

    fn url_prefix(&self) -> String {
        format!("/api/agents/{}", self.agent_name)
    }

    async fn workspace_relative_path(&self, path: &Path) -> Option<String> {
        let root = self.workspace_root.as_ref()?;
        let canonical_root = tokio::fs::canonicalize(root).await.ok()?;
        let canonical_path = tokio::fs::canonicalize(path).await.ok()?;
        let relative = canonical_path.strip_prefix(&canonical_root).ok()?;
        let relative_str = relative.to_str()?.replace(std::path::MAIN_SEPARATOR, "/");
        if residuum_workspace::access::is_blocked_path(&relative_str) {
            return None;
        }
        Some(relative_str)
    }

    pub async fn get(&self, id: &str) -> Option<(PathBuf, String, String)> {
        let entries = self.links.entries().await.read().await;
        let entry = entries.get(id)?;
        if entry.is_expired(Utc::now()) {
            return None;
        }
        Some((
            entry.path.clone(),
            entry.mime_type.clone(),
            entry.filename.clone(),
        ))
    }

    pub async fn sweep_expired(&self) -> (usize, usize) {
        self.links.sweep_expired().await
    }

    pub fn spawn_cleanup_task(&self) {
        let links = Arc::downgrade(&self.links);
        residuum_core::util::spawn_in_span(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(CLEANUP_INTERVAL_SECS));
            loop {
                interval.tick().await;
                let Some(links) = links.upgrade() else {
                    return;
                };
                let (removed, remaining) = links.sweep_expired().await;
                if removed > 0 {
                    tracing::debug!(removed, remaining, "swept expired file entries");
                }
            }
        });
    }
}

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

#[derive(Debug, serde::Deserialize)]
pub struct WorkspaceFileQuery {
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

    if residuum_workspace::access::is_blocked_path(relative) {
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
    let mime_type = residuum_bus::attachment::detect_mime_type(&canonical_target);
    Ok(respond_with_file(bytes, &mime_type, filename))
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
}

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

    async fn register_expired(registry: &FileRegistry, path: PathBuf) -> String {
        registry
            .register_until(
                path,
                "application/pdf".to_string(),
                "old.pdf".to_string(),
                Utc::now() - chrono::Duration::seconds(1),
            )
            .await
    }

    #[tokio::test]
    async fn an_entry_due_more_than_an_hour_out_counts_as_expired() {
        let registry = FileRegistry::new("scout");
        let far = registry
            .register_until(
                PathBuf::from("/tmp/far.pdf"),
                "application/pdf".to_string(),
                "far.pdf".to_string(),
                Utc::now() + chrono::Duration::seconds(FILE_TTL_SECS + 60),
            )
            .await;
        let fresh = registry
            .register(
                PathBuf::from("/tmp/fresh.pdf"),
                "application/pdf".to_string(),
                "fresh.pdf".to_string(),
            )
            .await;

        assert!(
            registry.get(&far).await.is_none(),
            "beyond one TTL is refused"
        );
        assert!(registry.get(&fresh).await.is_some(), "a fresh link serves");
        assert_eq!(registry.sweep_expired().await, (1, 1), "and is swept");
    }

    #[tokio::test]
    async fn sweep_removes_expired() {
        let registry = FileRegistry::new("scout");
        let id = register_expired(&registry, PathBuf::from("/tmp/old.pdf")).await;
        assert!(
            registry.get(&id).await.is_none(),
            "an expired link is refused"
        );
        let (removed, remaining) = registry.sweep_expired().await;
        assert_eq!(removed, 1, "one expired entry should be removed");
        assert_eq!(remaining, 0, "no entries should remain");
        assert!(registry.get(&id).await.is_none());
    }

    fn agent_registry(dir: &Path) -> FileRegistry {
        FileRegistry::for_agent("scout", &WorkspaceLayout::new(dir.join("scout")))
    }

    fn agent_dir_and_outside_file(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scout")).unwrap();
        std::fs::create_dir_all(dir.path().join("outside")).unwrap();
        let file = dir.path().join("outside/report.html");
        std::fs::write(&file, contents).unwrap();
        (dir, file)
    }

    #[tokio::test]
    async fn a_token_link_still_serves_after_the_registry_is_rebuilt_from_disk() {
        let (dir, file) = agent_dir_and_outside_file("<p>report</p>");
        let id = agent_registry(dir.path())
            .register(file, "text/html".to_string(), "report.html".to_string())
            .await;

        let restarted = agent_registry(dir.path());
        let resp = token_file(restarted, &id, tab_headers()).await;

        assert_eq!(resp.status(), StatusCode::OK, "served after the restart");
        assert_eq!(
            header_of(&resp, axum::http::header::CONTENT_SECURITY_POLICY),
            Some("sandbox allow-scripts allow-popups allow-downloads"),
            "still sandboxed"
        );
        assert_eq!(body_text(resp).await, "<p>report</p>");
    }

    #[tokio::test]
    async fn a_link_that_expired_on_disk_is_refused_and_swept_from_the_file() {
        let (dir, file) = agent_dir_and_outside_file("old");
        let registry = agent_registry(dir.path());
        let expired = register_expired(&registry, file.clone()).await;
        let live = registry
            .register(file, "text/plain".to_string(), "old.txt".to_string())
            .await;

        let restarted = agent_registry(dir.path());
        let tab = token_file(restarted.clone(), &expired, tab_headers()).await;
        assert_eq!(
            location(&tab),
            "/agent/scout?notice=file-link-expired",
            "an expired link lands on the expired notice"
        );

        assert_eq!(restarted.sweep_expired().await, (1, 1), "swept");
        let saved = std::fs::read_to_string(
            WorkspaceLayout::new(dir.path().join("scout")).file_links_json(),
        )
        .unwrap();
        assert!(!saved.contains(&expired), "gone from the file: {saved}");
        assert!(saved.contains(&live), "the live link stays: {saved}");
    }

    #[tokio::test]
    async fn a_token_link_whose_file_was_removed_lands_on_the_expired_notice_after_a_restart() {
        let (dir, file) = agent_dir_and_outside_file("soon gone");
        let id = agent_registry(dir.path())
            .register(
                file.clone(),
                "text/plain".to_string(),
                "gone.txt".to_string(),
            )
            .await;
        std::fs::remove_file(&file).unwrap();

        let tab = token_file(agent_registry(dir.path()), &id, tab_headers()).await;
        assert_eq!(location(&tab), "/agent/scout?notice=file-link-expired");
    }

    #[tokio::test]
    async fn an_unreadable_links_file_starts_empty_and_is_replaced_by_the_next_link() {
        let (dir, file) = agent_dir_and_outside_file("fresh");
        let store = WorkspaceLayout::new(dir.path().join("scout")).file_links_json();
        std::fs::write(&store, "{ not json").unwrap();

        let registry = agent_registry(dir.path());
        assert!(
            registry.get("anything").await.is_none(),
            "a corrupt file holds no links"
        );
        let id = registry
            .register(file, "text/plain".to_string(), "fresh.txt".to_string())
            .await;

        let resp = token_file(agent_registry(dir.path()), &id, fetch_headers()).await;
        assert_eq!(resp.status(), StatusCode::OK, "the new link was saved");
        assert_eq!(body_text(resp).await, "fresh");
    }

    #[tokio::test]
    async fn no_links_file_means_no_links_and_none_is_written_until_one_is_registered() {
        let (dir, _file) = agent_dir_and_outside_file("x");
        let store = WorkspaceLayout::new(dir.path().join("scout")).file_links_json();

        let registry = agent_registry(dir.path());
        assert!(registry.get("anything").await.is_none());
        assert_eq!(registry.sweep_expired().await, (0, 0));
        assert!(
            !store.exists(),
            "a sweep with nothing to remove writes nothing"
        );
    }

    #[tokio::test]
    async fn sweep_concurrent_register_does_not_underflow() {
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
    async fn url_for_reuses_an_unexpired_token_link_for_the_same_outside_file() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file_path = outside.path().join("scratch.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let registry = FileRegistry::new("scout").with_workspace_root(dir.path().to_path_buf());
        let first = registry
            .url_for(
                file_path.clone(),
                "text/plain".to_string(),
                "scratch.txt".to_string(),
            )
            .await;
        let second = registry
            .url_for(
                file_path,
                "text/plain".to_string(),
                "scratch.txt".to_string(),
            )
            .await;

        assert_eq!(first, second, "should reuse the active token link");
        assert_eq!(registry.sweep_expired().await, (0, 1));
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
                    residuum_bus::attachment::detect_mime_type(&outside.path().join(name)),
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
