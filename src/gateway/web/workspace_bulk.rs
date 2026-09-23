//! Bulk workspace reads: recursive tree listing and batch file read.
//!
//! Both endpoints exist so an artifact can load a whole subtree (or the
//! exact set of paths named by a change event) in one request instead of
//! one request per file or directory, which is what makes loading a large
//! folder through the cloud relay slow and, past the relay's in-flight
//! request cap, partially fail.

use std::path::{Path, PathBuf};

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Json;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::workspace::access::is_blocked_path;
use crate::workspace::version::{modified_unix_ms, version_token};

use super::ConfigApiState;
use super::workspace::{canonicalize_workspace_root, validate_workspace_path};

/// Maximum content size embedded in a tree or batch-read entry, per file
/// (1 MiB). Larger files still get their metadata, with `skipped`/`error`
/// set to `"too_large"`.
const PER_FILE_CONTENT_LIMIT_BYTES: u64 = 1024 * 1024;

/// Maximum number of entries a tree listing returns before it stops
/// walking and sets `listing_truncated`.
const TREE_ENTRY_LIMIT: usize = 20_000;

/// Maximum number of paths accepted in one batch-read request.
const BATCH_READ_PATH_LIMIT: usize = 1_000;

/// Serialized response budget for bulk reads, in bytes (8 MiB). Leaves
/// headroom under the relay tunnel's 10 MB response limit; once adding a
/// file's content would push the response past this, the content is
/// dropped (`skipped`/`error`: `"budget"`) but the entry's metadata stays.
const RESPONSE_BUDGET_BYTES: usize = 8 * 1024 * 1024;

/// One entry in a `GET /api/workspace/tree` listing.
#[derive(Debug, Serialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    entry_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    modified: u64,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped: Option<&'static str>,
}

/// Response body for `GET /api/workspace/tree`.
#[derive(Debug, Serialize)]
pub(super) struct TreeResponse {
    path: String,
    entries: Vec<TreeEntry>,
    listing_truncated: bool,
    content_truncated: bool,
}

/// Parsed and validated query parameters for `GET /api/workspace/tree`.
struct TreeParams {
    path: String,
    content: bool,
    glob: Vec<String>,
    depth: Option<u32>,
}

/// Parse `GET /api/workspace/tree`'s query string by hand.
///
/// `glob` repeats (`?glob=a&glob=b`), which axum's `Query` extractor can't
/// collect into a `Vec` (it deserializes via `serde_urlencoded`, which
/// treats a repeated key as a type error rather than a sequence). Reading
/// the raw query string with `url::form_urlencoded` sidesteps that without
/// pulling in another extractor crate.
fn parse_tree_query(raw: Option<&str>) -> Result<TreeParams, (StatusCode, String)> {
    let mut path = String::new();
    let mut content = false;
    let mut glob = Vec::new();
    let mut depth = None;

    if let Some(raw) = raw {
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            match key.as_ref() {
                "path" => path = value.into_owned(),
                "content" => content = matches!(value.as_ref(), "true" | "1"),
                "glob" => glob.push(value.into_owned()),
                "depth" => {
                    let parsed = value.parse::<u32>().map_err(|parse_err| {
                        (
                            StatusCode::BAD_REQUEST,
                            format!("invalid depth {value:?}: {parse_err}"),
                        )
                    })?;
                    depth = Some(parsed);
                }
                _ => {}
            }
        }
    }

    Ok(TreeParams {
        path,
        content,
        glob,
        depth,
    })
}

/// Compiled `glob` filters for a tree walk, split by whether each pattern
/// names a bare file name or a path relative to the walk's root.
struct CompiledGlobs {
    by_name: Option<GlobSet>,
    by_relative_path: Option<GlobSet>,
}

impl CompiledGlobs {
    /// Returns true if `basename` or `relative_path` matches any configured
    /// pattern.
    fn matches(&self, basename: &str, relative_path: &str) -> bool {
        self.by_name
            .as_ref()
            .is_some_and(|set| set.is_match(basename))
            || self
                .by_relative_path
                .as_ref()
                .is_some_and(|set| set.is_match(relative_path))
    }
}

/// Compile `glob` query values into matchers.
///
/// A pattern without `/` matches a file name at any depth; a pattern with
/// `/` matches the path relative to the walk's root. Both use
/// `literal_separator` so a bare `*` never crosses a `/` on its own —
/// matching ordinary glob and `.gitignore` intuition — while `**` still
/// spans directories.
fn compile_globs(patterns: &[String]) -> Result<Option<CompiledGlobs>, (StatusCode, String)> {
    if patterns.is_empty() {
        return Ok(None);
    }

    let mut by_name = GlobSetBuilder::new();
    let mut by_relative_path = GlobSetBuilder::new();
    let mut has_name = false;
    let mut has_relative_path = false;

    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("invalid glob pattern {pattern:?}: {e}"),
                )
            })?;

        if pattern.contains('/') {
            by_relative_path.add(glob);
            has_relative_path = true;
        } else {
            by_name.add(glob);
            has_name = true;
        }
    }

    let build =
        |builder: GlobSetBuilder, present: bool| -> Result<Option<GlobSet>, (StatusCode, String)> {
            if !present {
                return Ok(None);
            }
            builder.build().map(Some).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to compile glob patterns: {e}"),
                )
            })
        };

    Ok(Some(CompiledGlobs {
        by_name: build(by_name, has_name)?,
        by_relative_path: build(by_relative_path, has_relative_path)?,
    }))
}

/// One entry collected during the walk, before content is attached.
struct RawEntry {
    /// Workspace-relative path (root-relative `path` plus the walk suffix).
    path: String,
    entry_type: &'static str,
    size: Option<u64>,
    modified: u64,
    version: String,
    /// Absolute on-disk path, only set for files (used to read content).
    disk_path: Option<PathBuf>,
}

impl RawEntry {
    fn into_tree_entry(self) -> TreeEntry {
        TreeEntry {
            path: self.path,
            entry_type: self.entry_type,
            size: self.size,
            modified: self.modified,
            version: self.version,
            content: None,
            skipped: None,
        }
    }
}

/// Mutable state threaded through the recursive walk.
struct WalkState<'a> {
    root_relative: &'a str,
    depth_limit: Option<u32>,
    globs: Option<&'a CompiledGlobs>,
    entries: Vec<RawEntry>,
    listing_truncated: bool,
}

impl WalkState<'_> {
    /// Build the workspace-relative path for `sub` (relative to the walk's
    /// root, `/`-separated regardless of platform).
    fn workspace_relative(&self, sub: &str) -> String {
        if self.root_relative.is_empty() {
            sub.to_string()
        } else if sub.is_empty() {
            self.root_relative.to_string()
        } else {
            format!("{}/{sub}", self.root_relative)
        }
    }
}

/// Recursively walk `disk_dir`, appending entries to `state`.
///
/// `rel_sub` is the path of `disk_dir` relative to the walk's root
/// (`""` for the root itself), and `depth` is the depth level of the
/// entries this call will add (`1` for the root's direct children).
/// Symlinks are never followed and never listed. Blocked paths (and
/// anything under a blocked directory) are omitted entirely.
fn walk_dir(
    disk_dir: &Path,
    rel_sub: &str,
    depth: u32,
    state: &mut WalkState,
) -> anyhow::Result<()> {
    if state.entries.len() >= TREE_ENTRY_LIMIT {
        state.listing_truncated = true;
        return Ok(());
    }

    let mut dir_entries: Vec<std::fs::DirEntry> =
        std::fs::read_dir(disk_dir)?.collect::<Result<_, _>>()?;
    dir_entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in dir_entries {
        if state.entries.len() >= TREE_ENTRY_LIMIT {
            state.listing_truncated = true;
            break;
        }

        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        let sub_path = if rel_sub.is_empty() {
            name.clone()
        } else {
            format!("{rel_sub}/{name}")
        };
        let workspace_relative = state.workspace_relative(&sub_path);
        if is_blocked_path(&workspace_relative) {
            continue;
        }

        if file_type.is_dir() {
            let metadata = entry.metadata()?;
            if state.globs.is_none() {
                state.entries.push(RawEntry {
                    path: workspace_relative,
                    entry_type: "directory",
                    size: None,
                    modified: modified_unix_ms(&metadata),
                    version: version_token(&metadata),
                    disk_path: None,
                });
            }

            let child_depth = depth + 1;
            if state.depth_limit.is_none_or(|limit| child_depth <= limit) {
                walk_dir(&entry.path(), &sub_path, child_depth, state)?;
            }
        } else if file_type.is_file() {
            let include = state
                .globs
                .is_none_or(|globs| globs.matches(&name, &sub_path));
            if !include {
                continue;
            }

            let metadata = entry.metadata()?;
            state.entries.push(RawEntry {
                path: workspace_relative,
                entry_type: "file",
                size: Some(metadata.len()),
                modified: modified_unix_ms(&metadata),
                version: version_token(&metadata),
                disk_path: Some(entry.path()),
            });
        }
    }

    Ok(())
}

/// Serialized JSON length of `value`, plus one byte for its separator in
/// the surrounding array — used to track the response budget against the
/// same escaped form that ships on the wire, not raw byte counts.
fn json_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_string(value).map_or(0, |s| s.len() + 1)
}

/// Read `disk_path`'s content for a tree or batch-read entry, honoring the
/// per-file size limit and UTF-8 requirement.
///
/// Returns `(content, skip_reason)`; exactly one is `Some`, unless the read
/// itself failed (a benign race with a concurrent delete), in which case
/// both are `None` and the caller falls back to metadata only.
fn read_file_content(
    disk_path: &Path,
    path_for_log: &str,
    size: u64,
) -> (Option<String>, Option<&'static str>) {
    if size > PER_FILE_CONTENT_LIMIT_BYTES {
        return (None, Some("too_large"));
    }

    match std::fs::read(disk_path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => (Some(text), None),
            Err(_) => (None, Some("binary")),
        },
        Err(e) => {
            tracing::warn!(
                path = %path_for_log,
                error = %e,
                "failed to read workspace file content for bulk read; returning metadata only"
            );
            (None, None)
        }
    }
}

/// Attach content to `raw.entries()` in path order, honoring the per-file
/// and whole-response budgets. Returns the finished entries and whether any
/// file's content was dropped for budget.
fn attach_tree_content(
    mut raw_entries: Vec<RawEntry>,
    content_requested: bool,
) -> (Vec<TreeEntry>, bool) {
    raw_entries.sort_by(|a, b| a.path.cmp(&b.path));

    if !content_requested {
        return (
            raw_entries
                .into_iter()
                .map(RawEntry::into_tree_entry)
                .collect(),
            false,
        );
    }

    let mut used_bytes = 0_usize;
    let mut content_truncated = false;
    let mut out = Vec::with_capacity(raw_entries.len());

    for raw in raw_entries {
        let mut entry = if raw.entry_type == "file" {
            let disk_path = raw.disk_path.clone();
            let size = raw.size.unwrap_or(0);
            // A file's raw size is a lower bound on its escaped JSON size, so
            // one that can't fit is skipped without reading it from disk.
            let cannot_fit = size <= PER_FILE_CONTENT_LIMIT_BYTES
                && usize::try_from(size).map_or(true, |len| {
                    used_bytes.saturating_add(len) > RESPONSE_BUDGET_BYTES
                });
            let (content, skipped) = if cannot_fit {
                content_truncated = true;
                (None, Some("budget"))
            } else {
                disk_path
                    .as_deref()
                    .map_or((None, None), |p| read_file_content(p, &raw.path, size))
            };
            TreeEntry {
                path: raw.path,
                entry_type: raw.entry_type,
                size: raw.size,
                modified: raw.modified,
                version: raw.version,
                content,
                skipped,
            }
        } else {
            raw.into_tree_entry()
        };

        if entry.content.is_some() {
            let with_content_len = json_len(&entry);
            if used_bytes + with_content_len > RESPONSE_BUDGET_BYTES {
                entry.content = None;
                entry.skipped = Some("budget");
                content_truncated = true;
            }
        }

        used_bytes += json_len(&entry);
        out.push(entry);
    }

    (out, content_truncated)
}

/// Walk the directory at `root_disk` (workspace-relative path
/// `root_relative`) and build the tree response.
fn build_tree(
    root_disk: &Path,
    root_relative: &str,
    depth_limit: Option<u32>,
    globs: Option<&CompiledGlobs>,
    content_requested: bool,
) -> anyhow::Result<TreeResponse> {
    let mut state = WalkState {
        root_relative,
        depth_limit,
        globs,
        entries: Vec::new(),
        listing_truncated: false,
    };

    if state.depth_limit.is_none_or(|limit| limit >= 1) {
        walk_dir(root_disk, "", 1, &mut state)?;
    }

    let listing_truncated = state.listing_truncated;
    let (entries, content_truncated) = attach_tree_content(state.entries, content_requested);

    Ok(TreeResponse {
        path: root_relative.to_string(),
        entries,
        listing_truncated,
        content_truncated,
    })
}

/// `GET /api/workspace/tree` — recursively list a workspace directory.
///
/// Returns a flat, path-sorted list of every file and directory under
/// `path` (the whole workspace when omitted). Symlinks below the root are
/// never followed and never listed; blocked paths (per the workspace access
/// policy) are absent. With `content=true`, files up to 1 MiB of valid
/// UTF-8 carry their text, within an 8 MiB response budget; entries that
/// don't fit carry `skipped` instead, with their metadata intact.
///
/// # Errors
/// Returns 403 for a blocked or workspace-escaping `path`, 404 if it
/// doesn't exist, 400 if it exists but isn't a directory or a `glob`
/// pattern doesn't compile, and 500 if the walk itself fails unexpectedly.
pub(super) async fn api_workspace_tree(
    State(state): State<ConfigApiState>,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<TreeResponse>, (StatusCode, String)> {
    let params = parse_tree_query(raw_query.as_deref())?;

    if is_blocked_path(&params.path) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    let root_disk = if params.path.is_empty() {
        canonicalize_workspace_root(&state.workspace_dir).await?
    } else {
        validate_workspace_path(&state.workspace_dir, &params.path).await?
    };

    let root_metadata = tokio::fs::metadata(&root_disk).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to stat {}: {e}", params.path),
        )
    })?;
    if !root_metadata.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{} is not a directory", params.path),
        ));
    }

    let globs = compile_globs(&params.glob)?;

    let root_relative = params.path.clone();
    let response = tokio::task::spawn_blocking(move || {
        build_tree(
            &root_disk,
            &root_relative,
            params.depth,
            globs.as_ref(),
            params.content,
        )
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("tree walk task failed: {e}"),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to walk directory: {e}"),
        )
    })?;

    Ok(Json(response))
}

/// Request body for `POST /api/workspace/read`.
#[derive(Deserialize)]
pub(super) struct BatchReadRequest {
    paths: Vec<String>,
}

/// One entry in a `POST /api/workspace/read` response, in request order.
#[derive(Debug, Serialize)]
struct BatchFileResult {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

impl BatchFileResult {
    /// A structural failure (`not_found`, `blocked`, `is_directory`): the
    /// path carries no metadata because none was resolved, or none applies.
    fn structural_error(path: String, error: &'static str) -> Self {
        Self {
            path,
            size: None,
            modified: None,
            version: None,
            content: None,
            error: Some(error),
        }
    }

    /// A content-specific skip (`binary`, `too_large`, `budget`): the file
    /// exists and was stat'd, so its metadata is included alongside the
    /// error naming why its content isn't.
    fn content_error(
        path: String,
        size: u64,
        modified: u64,
        version: String,
        error: &'static str,
    ) -> Self {
        Self {
            path,
            size: Some(size),
            modified: Some(modified),
            version: Some(version),
            content: None,
            error: Some(error),
        }
    }

    fn ok(path: String, size: u64, modified: u64, version: String, content: String) -> Self {
        Self {
            path,
            size: Some(size),
            modified: Some(modified),
            version: Some(version),
            content: Some(content),
            error: None,
        }
    }
}

/// Response body for `POST /api/workspace/read`.
#[derive(Debug, Serialize)]
pub(super) struct BatchReadResponse {
    files: Vec<BatchFileResult>,
    content_truncated: bool,
}

/// Resolve and classify one requested path, without content or budget.
///
/// A path that is blocked, or that canonicalizes outside the workspace
/// (paths that escape count as blocked, same as any other blocked path),
/// answers `blocked`. A path that doesn't exist, or that some other I/O
/// error prevented resolving (logged for diagnosis), answers `not_found`.
fn classify_batch_path(
    workspace_dir: &Path,
    canonical_root: &Path,
    relative: &str,
) -> BatchFileResult {
    if is_blocked_path(relative) {
        return BatchFileResult::structural_error(relative.to_string(), "blocked");
    }

    let target = workspace_dir.join(relative);
    let canonical_target = match std::fs::canonicalize(&target) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return BatchFileResult::structural_error(relative.to_string(), "not_found");
        }
        Err(e) => {
            tracing::warn!(
                path = %relative,
                error = %e,
                "failed to resolve workspace path for batch read"
            );
            return BatchFileResult::structural_error(relative.to_string(), "not_found");
        }
    };

    if !canonical_target.starts_with(canonical_root) {
        return BatchFileResult::structural_error(relative.to_string(), "blocked");
    }

    let metadata = match std::fs::metadata(&canonical_target) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                path = %relative,
                error = %e,
                "failed to stat workspace path for batch read"
            );
            return BatchFileResult::structural_error(relative.to_string(), "not_found");
        }
    };

    if metadata.is_dir() {
        return BatchFileResult::structural_error(relative.to_string(), "is_directory");
    }

    let size = metadata.len();
    let modified = modified_unix_ms(&metadata);
    let version = version_token(&metadata);

    let (content, skipped) = read_file_content(&canonical_target, relative, size);
    match (content, skipped) {
        (Some(text), _) => BatchFileResult::ok(relative.to_string(), size, modified, version, text),
        (None, Some(reason)) => {
            BatchFileResult::content_error(relative.to_string(), size, modified, version, reason)
        }
        // The read itself raced with a concurrent delete: no content, no
        // specific reason. Report it as gone rather than inventing a
        // status outside the documented error set.
        (None, None) => BatchFileResult::structural_error(relative.to_string(), "not_found"),
    }
}

/// Resolve, read, and budget one requested path.
fn resolve_and_read_one(
    workspace_dir: &Path,
    canonical_root: &Path,
    relative: &str,
    used_bytes: &mut usize,
) -> BatchFileResult {
    let mut result = classify_batch_path(workspace_dir, canonical_root, relative);

    if result.content.is_some() {
        let with_content_len = json_len(&result);
        if *used_bytes + with_content_len > RESPONSE_BUDGET_BYTES {
            result.content = None;
            result.error = Some("budget");
        }
    }

    *used_bytes += json_len(&result);
    result
}

/// Read every path in `paths`, in order, within the shared response budget.
fn batch_read(workspace_dir: &Path, paths: &[String]) -> (Vec<BatchFileResult>, bool) {
    let canonical_root = match std::fs::canonicalize(workspace_dir) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(
                error = %e,
                workspace = %workspace_dir.display(),
                "failed to resolve workspace root for batch read"
            );
            let files = paths
                .iter()
                .map(|p| BatchFileResult::structural_error(p.clone(), "not_found"))
                .collect();
            return (files, false);
        }
    };

    let mut used_bytes = 0_usize;
    let mut content_truncated = false;
    let mut results = Vec::with_capacity(paths.len());

    for relative in paths {
        let result =
            resolve_and_read_one(workspace_dir, &canonical_root, relative, &mut used_bytes);
        if result.error == Some("budget") {
            content_truncated = true;
        }
        results.push(result);
    }

    (results, content_truncated)
}

/// `POST /api/workspace/read` — read a chosen set of workspace files.
///
/// Typically the paths named by a change event, so an artifact can refresh
/// exactly what changed in one request. Results are returned in request
/// order; one bad path never fails the request — it gets a per-file
/// `error` instead. The same per-file (1 MiB) and response (8 MiB) budgets
/// as the tree endpoint apply.
///
/// # Errors
/// Returns 400 if more than 1,000 paths are requested.
pub(super) async fn api_workspace_read(
    State(state): State<ConfigApiState>,
    Json(req): Json<BatchReadRequest>,
) -> Result<Json<BatchReadResponse>, (StatusCode, String)> {
    if req.paths.len() > BATCH_READ_PATH_LIMIT {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "at most {BATCH_READ_PATH_LIMIT} paths may be requested at once ({} given)",
                req.paths.len()
            ),
        ));
    }

    let workspace_dir = state.workspace_dir.clone();
    let (files, content_truncated) =
        tokio::task::spawn_blocking(move || batch_read(&workspace_dir, &req.paths))
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("batch read task failed: {e}"),
                )
            })?;

    Ok(Json(BatchReadResponse {
        files,
        content_truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(ws_dir: PathBuf) -> ConfigApiState {
        super::super::ConfigApiState {
            config_dir: ws_dir.clone(),
            workspace_dir: ws_dir,
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    async fn tree(state: &ConfigApiState, query: &str) -> TreeResponse {
        let params = parse_tree_query(Some(query)).unwrap();
        let root_disk = if params.path.is_empty() {
            canonicalize_workspace_root(&state.workspace_dir)
                .await
                .unwrap()
        } else {
            validate_workspace_path(&state.workspace_dir, &params.path)
                .await
                .unwrap()
        };
        let globs = compile_globs(&params.glob).unwrap();
        build_tree(
            &root_disk,
            &params.path,
            params.depth,
            globs.as_ref(),
            params.content,
        )
        .unwrap()
    }

    async fn tree_err(state: &ConfigApiState, query: &str) -> (StatusCode, String) {
        api_workspace_tree(State(state.clone()), RawQuery(Some(query.to_string())))
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn recursive_listing_finds_nested_entries() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("wiki/sub"))
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("wiki/a.md"), "a")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("wiki/sub/b.md"), "b")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let response = tree(&state, "path=wiki").await;
        let paths: Vec<&str> = response.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["wiki/a.md", "wiki/sub", "wiki/sub/b.md"]);
        assert!(!response.listing_truncated);
        assert!(!response.content_truncated);
    }

    #[tokio::test]
    async fn name_glob_matches_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("notes/sub"))
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("notes/a.md"), "a")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("notes/sub/b.md"), "b")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("notes/c.txt"), "c")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let response = tree(&state, "glob=*.md").await;
        let paths: Vec<&str> = response.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["notes/a.md", "notes/sub/b.md"]);
        for entry in &response.entries {
            assert_eq!(
                entry.entry_type, "file",
                "glob filtering must drop directory entries"
            );
        }
    }

    #[tokio::test]
    async fn path_glob_matches_only_relative_to_root() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("notes/sub"))
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("notes/a.md"), "a")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("notes/sub/b.md"), "b")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        // "notes/*.md" is relative to the walk's root ("" here), so it only
        // matches a file directly inside a top-level "notes/", not one
        // nested further inside "notes/sub/".
        let response = tree(&state, "glob=notes%2F*.md").await;
        let paths: Vec<&str> = response.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["notes/a.md"]);
    }

    #[tokio::test]
    async fn depth_limit_stops_recursion() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("a/b")).await.unwrap();
        tokio::fs::write(ws_dir.join("top.md"), "t").await.unwrap();
        tokio::fs::write(ws_dir.join("a/mid.md"), "m")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("a/b/deep.md"), "d")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let response = tree(&state, "depth=1").await;
        let paths: Vec<&str> = response.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["a", "top.md"]);
    }

    #[tokio::test]
    async fn blocked_paths_and_symlinks_are_absent() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("vectors.db"), "bin")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("real.md"), "real")
            .await
            .unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(ws_dir.join("real.md"), ws_dir.join("link.md")).unwrap();
        }
        let state = make_state(ws_dir);

        let response = tree(&state, "").await;
        let paths: Vec<&str> = response.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(!paths.contains(&"vectors.db"));
        #[cfg(unix)]
        assert!(!paths.contains(&"link.md"));
        assert!(paths.contains(&"real.md"));
    }

    #[tokio::test]
    async fn binary_and_oversized_files_carry_skipped_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("binary.dat"), [0xff, 0xfe, 0x00, 0xff])
            .await
            .unwrap();
        tokio::fs::write(
            ws_dir.join("huge.md"),
            "a".repeat(usize::try_from(PER_FILE_CONTENT_LIMIT_BYTES).unwrap() + 1),
        )
        .await
        .unwrap();
        let state = make_state(ws_dir);

        let response = tree(&state, "content=true").await;
        let find = |name: &str| response.entries.iter().find(|e| e.path == name).unwrap();

        let binary = find("binary.dat");
        assert_eq!(binary.skipped, Some("binary"));
        assert!(binary.content.is_none());
        assert!(
            binary.size.is_some(),
            "skipped entries still carry metadata"
        );

        let huge = find("huge.md");
        assert_eq!(huge.skipped, Some("too_large"));
        assert!(huge.content.is_none());
        assert!(huge.size.is_some());
    }

    #[tokio::test]
    async fn budget_truncation_keeps_response_under_eight_mebibytes_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();

        // Nine ~1 MiB files comfortably exceed the 8 MiB response budget
        // once their content is embedded and JSON-escaped.
        let file_count = 9;
        let file_size = usize::try_from(PER_FILE_CONTENT_LIMIT_BYTES).unwrap();
        for i in 0..file_count {
            tokio::fs::write(ws_dir.join(format!("file{i:02}.md")), "a".repeat(file_size))
                .await
                .unwrap();
        }
        let state = make_state(ws_dir);

        let response = api_workspace_tree(State(state), RawQuery(Some("content=true".to_string())))
            .await
            .unwrap()
            .0;

        assert_eq!(
            response.entries.len(),
            file_count,
            "every file keeps its metadata entry"
        );
        assert!(
            response.content_truncated,
            "at least one file's content must be dropped for budget"
        );
        assert!(
            response.entries.iter().any(|e| e.skipped == Some("budget")),
            "a dropped entry is marked skipped: budget"
        );
        for entry in &response.entries {
            assert!(
                entry.size.is_some(),
                "every entry keeps its size even when content is skipped"
            );
            assert!(
                !entry.version.is_empty(),
                "every entry keeps its version even when content is skipped"
            );
        }

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(
            serialized.len() < RESPONSE_BUDGET_BYTES,
            "serialized response ({} bytes) must stay under the 8 MiB budget",
            serialized.len()
        );
    }

    #[tokio::test]
    async fn listing_truncated_at_the_entry_cap() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        for i in 0..(TREE_ENTRY_LIMIT + 5) {
            std::fs::write(ws_dir.join(format!("f{i:05}.md")), "").unwrap();
        }
        let state = make_state(ws_dir);

        let response = tree(&state, "").await;
        assert_eq!(response.entries.len(), TREE_ENTRY_LIMIT);
        assert!(response.listing_truncated);
    }

    #[tokio::test]
    async fn tree_root_validation_answers_403_404_and_400() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("file.md"), "x").await.unwrap();
        let state = make_state(ws_dir);

        let (blocked_status, _) = tree_err(&state, "path=.index").await;
        assert_eq!(blocked_status, StatusCode::FORBIDDEN);

        let (missing_status, _) = tree_err(&state, "path=missing").await;
        assert_eq!(missing_status, StatusCode::NOT_FOUND);

        let (not_dir_status, _) = tree_err(&state, "path=file.md").await;
        assert_eq!(not_dir_status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn batch_read_preserves_order_and_reports_per_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("a.md"), "hello")
            .await
            .unwrap();
        tokio::fs::create_dir(ws_dir.join("adir")).await.unwrap();
        // A real file outside the workspace, so the escape actually resolves
        // (rather than failing to canonicalize) and exercises the
        // prefix-mismatch branch of the blocked check.
        tokio::fs::write(dir.path().join("escape.md"), "secret")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let response = api_workspace_read(
            State(state),
            Json(BatchReadRequest {
                paths: vec![
                    "a.md".to_string(),
                    "gone.md".to_string(),
                    "adir".to_string(),
                    "vectors.db".to_string(),
                    "../escape.md".to_string(),
                ],
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(response.files.len(), 5);
        let mut files = response.files.into_iter();
        let a = files.next().unwrap();
        assert_eq!(a.path, "a.md");
        assert_eq!(a.content.as_deref(), Some("hello"));
        let gone = files.next().unwrap();
        assert_eq!(gone.path, "gone.md");
        assert_eq!(gone.error, Some("not_found"));
        let adir = files.next().unwrap();
        assert_eq!(adir.path, "adir");
        assert_eq!(adir.error, Some("is_directory"));
        let db = files.next().unwrap();
        assert_eq!(db.path, "vectors.db");
        assert_eq!(db.error, Some("blocked"));
        let escape = files.next().unwrap();
        assert_eq!(escape.path, "../escape.md");
        assert_eq!(
            escape.error,
            Some("blocked"),
            "an escaping path counts as blocked"
        );
        assert!(files.next().is_none());
        assert!(!response.content_truncated);
    }

    #[tokio::test]
    async fn batch_read_rejects_over_the_path_limit() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let paths = (0..=BATCH_READ_PATH_LIMIT)
            .map(|i| format!("f{i}.md"))
            .collect();
        let err = api_workspace_read(State(state), Json(BatchReadRequest { paths }))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn batch_read_budget_truncation_keeps_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let file_size = usize::try_from(PER_FILE_CONTENT_LIMIT_BYTES).unwrap();
        let mut paths = Vec::new();
        for i in 0..9 {
            let name = format!("file{i:02}.md");
            tokio::fs::write(ws_dir.join(&name), "a".repeat(file_size))
                .await
                .unwrap();
            paths.push(name);
        }
        let state = make_state(ws_dir);

        let response = api_workspace_read(State(state), Json(BatchReadRequest { paths }))
            .await
            .unwrap()
            .0;

        assert_eq!(response.files.len(), 9);
        assert!(response.content_truncated);
        assert!(response.files.iter().any(|f| f.error == Some("budget")));
        for file in &response.files {
            assert!(
                file.size.is_some(),
                "every entry keeps its metadata even when content is skipped"
            );
        }

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.len() < RESPONSE_BUDGET_BYTES);
    }
}
