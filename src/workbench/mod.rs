//! Workbench: interactive artifacts the agent builds for the user.
//!
//! An artifact is either a single page, `<workspace>/workbench/<name>.html`, or a
//! folder, `<workspace>/workbench/<name>/` with an `index.html` and any other
//! files it loads. `<name>` is kebab-case. The artifacts listener
//! ([`server`]) serves them on their own origin at `/<name>/`; the web UI
//! lists them at `/workbench` and shows one at `/workbench/<name>`. Files
//! beside an artifact that share its `<name>.` prefix (for example
//! `<name>.state.json`) are that artifact's saved data: not part of the artifact, not
//! watched for reloads, and deleted with it.

pub(crate) mod server;
pub(crate) mod watcher;

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::features;
use crate::gateway::protocol::ArtifactSummary;
use crate::update;

/// Only the start of a page is scanned for its `<title>` when listing.
const TITLE_SCAN_BYTES: usize = 64 * 1024;

const MAX_ARTIFACT_NAME_LEN: usize = 64;

/// The SDK injected into every served HTML file.
const SDK_JS: &str = include_str!("../../assets/workbench/sdk.js");

/// Whether `name` is a valid artifact name: lowercase ASCII letters and digits in
/// hyphen-separated words, at most 64 characters. Names carry no path
/// separators or dots, so a valid name always resolves inside the workbench.
#[must_use]
pub(crate) fn is_valid_artifact_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ARTIFACT_NAME_LEN
        && name.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Where an artifact lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArtifactKind {
    /// `<name>.html`.
    Page(PathBuf),
    /// `<name>/`, entered through `<name>/index.html`.
    Folder(PathBuf),
}

/// An artifact found in the workbench directory.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredArtifact {
    pub name: String,
    pub kind: ArtifactKind,
    /// Newest modification time among the artifact's files.
    pub modified: Option<SystemTime>,
    /// Total size of the artifact's files.
    pub size: u64,
    /// Number of files in the artifact.
    pub files: usize,
}

impl DiscoveredArtifact {
    /// The HTML page the artifact opens with.
    fn entry_page(&self) -> PathBuf {
        match &self.kind {
            ArtifactKind::Page(path) => path.clone(),
            ArtifactKind::Folder(dir) => dir.join("index.html"),
        }
    }
}

/// Find every artifact in `dir`. A missing directory is an empty workbench.
/// Symlinks are skipped, so an artifact can never expose files from outside the
/// workbench. When both `<name>.html` and `<name>/` exist, the folder wins.
///
/// # Errors
/// Returns an error if the directory exists but cannot be read.
pub(crate) async fn discover_artifacts(dir: &Path) -> std::io::Result<Vec<DiscoveredArtifact>> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut artifacts: Vec<DiscoveredArtifact> = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let file_type = entry.file_type().await?;
        let found = if file_type.is_dir() && is_valid_artifact_name(&file_name) {
            if !tokio::fs::symlink_metadata(entry.path().join("index.html"))
                .await
                .is_ok_and(|m| m.is_file())
            {
                continue;
            }
            let (modified, size, files) = folder_stats(entry.path()).await?;
            DiscoveredArtifact {
                name: file_name,
                kind: ArtifactKind::Folder(entry.path()),
                modified,
                size,
                files,
            }
        } else if file_type.is_file()
            && let Some(name) = file_name
                .strip_suffix(".html")
                .filter(|stem| is_valid_artifact_name(stem))
        {
            let metadata = entry.metadata().await?;
            DiscoveredArtifact {
                name: name.to_string(),
                kind: ArtifactKind::Page(entry.path()),
                modified: metadata.modified().ok(),
                size: metadata.len(),
                files: 1,
            }
        } else {
            continue;
        };

        match artifacts.iter_mut().find(|t| t.name == found.name) {
            Some(existing) if matches!(found.kind, ArtifactKind::Folder(_)) => *existing = found,
            Some(_) => {}
            None => artifacts.push(found),
        }
    }
    Ok(artifacts)
}

/// Newest modification time, total size, and file count of a folder artifact,
/// skipping symlinks. Unbounded: a workbench folder is a hand-built tool, not
/// user-uploaded content, so there is no realistic file count that needs
/// capping, and the watcher's change detection depends on an honest count and
/// size to notice edits past whatever a cap would have cut off at.
async fn folder_stats(root: PathBuf) -> std::io::Result<(Option<SystemTime>, u64, usize)> {
    let mut newest: Option<SystemTime> = None;
    let mut size = 0_u64;
    let mut files = 0_usize;
    let mut pending = vec![root];
    while let Some(dir) = pending.pop() {
        let mut read_dir = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let metadata = entry.metadata().await?;
                size += metadata.len();
                files += 1;
                if let Ok(m) = metadata.modified() {
                    newest = Some(newest.map_or(m, |n| n.max(m)));
                }
            }
        }
    }
    Ok((newest, size, files))
}

/// List the artifacts in `dir` for the web UI, most recently modified first.
///
/// # Errors
/// Returns an error if the directory exists but cannot be read.
pub(crate) async fn list_artifacts(dir: &Path) -> std::io::Result<Vec<ArtifactSummary>> {
    let mut artifacts = Vec::new();
    for artifact in discover_artifacts(dir).await? {
        let title = match read_title(&artifact.entry_page()).await {
            Ok(title) => title,
            Err(e) => {
                tracing::warn!(artifact = %artifact.name, error = %e, "failed to read workbench artifact title");
                None
            }
        };
        artifacts.push(ArtifactSummary {
            title: title.unwrap_or_else(|| artifact.name.clone()),
            modified_at: artifact
                .modified
                .map_or(DateTime::<Utc>::UNIX_EPOCH, DateTime::<Utc>::from),
            size: artifact.size,
            name: artifact.name,
        });
    }
    artifacts.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(artifacts)
}

async fn read_title(path: &Path) -> std::io::Result<Option<String>> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path).await?;
    let mut head = Vec::with_capacity(TITLE_SCAN_BYTES);
    file.take(TITLE_SCAN_BYTES as u64)
        .read_to_end(&mut head)
        .await?;
    Ok(extract_title(&String::from_utf8_lossy(&head)))
}

/// The trimmed text of the page's first `<title>` element, with the common
/// character references decoded. `None` when there is no non-empty title.
#[must_use]
pub(crate) fn extract_title(html: &str) -> Option<String> {
    // ASCII lowercasing keeps byte offsets identical to the original.
    let lower = html.to_ascii_lowercase();
    let open = find_tag(&lower, "title", 0)?;
    let content_start = open + lower.get(open..)?.find('>')? + 1;
    let content_end = content_start + lower.get(content_start..)?.find("</title")?;
    let raw = html.get(content_start..content_end)?;
    let title = decode_entities(
        raw.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .as_str(),
    );
    (!title.is_empty()).then_some(title)
}

/// Byte offset of the first `<tag` opening in `lower` at or after `from`,
/// where the tag name is followed by `>`, `/`, or whitespace (so `<head` does
/// not match `<header`).
fn find_tag(lower: &str, tag: &str, from: usize) -> Option<usize> {
    let needle = format!("<{tag}");
    let mut search_from = from;
    loop {
        let at = search_from + lower.get(search_from..)?.find(&needle)?;
        let after = lower.as_bytes().get(at + needle.len()).copied();
        if matches!(after, Some(b'>' | b'/')) || after.is_some_and(|b| b.is_ascii_whitespace()) {
            return Some(at);
        }
        search_from = at + needle.len();
    }
}

fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Serializes `value` as JSON for embedding inside a `<script>` block, escaping
/// any `</` sequence so embedded content can never close the tag early.
fn embed_as_script_json<T: serde::Serialize + ?Sized>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    json.replace("</", "<\\/")
}

/// Insert the SDK `<script>` so it runs before any of the page's own scripts:
/// right after the `<head>` open tag, else after `<html>`, else after the
/// doctype, else at the very start. The script embeds the artifact's own
/// name, this build's version, and its feature list, which the SDK exposes as
/// `residuum.artifact`, `residuum.version`, and `residuum.features`.
#[must_use]
pub(crate) fn inject_sdk(html: &str, artifact: &str, version: &str, features: &[&str]) -> String {
    let lower = html.to_ascii_lowercase();
    let insert_at = ["head", "html", "!doctype"]
        .iter()
        .find_map(|tag| {
            let open = find_tag(&lower, tag, 0)?;
            Some(open + lower.get(open..)?.find('>')? + 1)
        })
        .unwrap_or(0);

    let context = format!(
        "const __RESIDUUM_ARTIFACT__={};const __RESIDUUM_VERSION__={};const __RESIDUUM_FEATURES__={};",
        embed_as_script_json(artifact),
        embed_as_script_json(version),
        embed_as_script_json(features),
    );
    // The block scopes the context constants to the SDK, keeping them out of
    // the page's global scope where an artifact's own names could collide.
    let script = format!("<script>{{{context}{SDK_JS}}}</script>");
    let mut out = String::with_capacity(html.len() + script.len());
    out.push_str(html.get(..insert_at).unwrap_or_default());
    out.push_str(&script);
    out.push_str(html.get(insert_at..).unwrap_or(html));
    out
}

/// Why an artifact file could not be served.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ArtifactFileError {
    #[error("no workbench artifact named {0:?}")]
    NoSuchArtifact(String),
    #[error("workbench artifact {artifact:?} has no file {path:?}")]
    NotFound { artifact: String, path: String },
    #[error("failed to read {path:?} in workbench artifact {artifact:?}: {source}")]
    Io {
        artifact: String,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// An artifact file's body, ready to serve.
///
/// HTML needs the SDK injected, so it's read fully and rewritten in memory.
/// Everything else is streamed straight from disk, so serving a large local
/// artifact file (an exported dataset, a video) doesn't buffer it whole —
/// there is no size cap on serving it locally. A relay tunnel session
/// forwarding this response applies its own size limit
/// (`tunnel::forward_http::MAX_RESPONSE_SIZE`) on that path, not this one.
pub(crate) enum ArtifactBody {
    Bytes(Vec<u8>),
    File(tokio::fs::File),
}

/// An artifact file ready to serve.
pub(crate) struct ArtifactFile {
    pub body: ArtifactBody,
    pub content_type: String,
}

/// Read file `rest` of artifact `name` (`""` for the artifact's page). HTML gets the
/// SDK injected.
///
/// `rest` is a `/`-separated path relative to a folder artifact. A trailing `/`
/// means that directory's `index.html`. Empty, `.`, and `..` segments are
/// refused (a dot-prefixed filename like `.env.example` is not), and the
/// resolved file must stay inside the artifact's folder after symlinks. A
/// single-page artifact has no other files.
///
/// # Errors
/// Returns [`ArtifactFileError`] when the artifact or file does not exist or
/// cannot be read.
pub(crate) async fn read_artifact_file(
    dir: &Path,
    name: &str,
    rest: &str,
) -> Result<ArtifactFile, ArtifactFileError> {
    let artifact = discover_artifacts(dir)
        .await
        .ok()
        .and_then(|artifacts| artifacts.into_iter().find(|t| t.name == name))
        .ok_or_else(|| ArtifactFileError::NoSuchArtifact(name.to_string()))?;
    let not_found = || ArtifactFileError::NotFound {
        artifact: name.to_string(),
        path: rest.to_string(),
    };

    let path = match &artifact.kind {
        ArtifactKind::Page(page) if rest.is_empty() || rest == "index.html" => page.clone(),
        ArtifactKind::Page(_) => return Err(not_found()),
        ArtifactKind::Folder(root) => resolve_in_folder(root, rest).await.ok_or_else(not_found)?,
    };

    let io_err = |source| ArtifactFileError::Io {
        artifact: name.to_string(),
        path: rest.to_string(),
        source,
    };
    let metadata = tokio::fs::metadata(&path).await.map_err(io_err)?;
    if !metadata.is_file() {
        return Err(not_found());
    }

    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    if mime.essence_str() == "text/html" {
        let bytes = tokio::fs::read(&path).await.map_err(io_err)?;
        Ok(ArtifactFile {
            body: ArtifactBody::Bytes(
                inject_sdk(
                    &String::from_utf8_lossy(&bytes),
                    name,
                    update::CURRENT_VERSION,
                    features::FEATURES,
                )
                .into_bytes(),
            ),
            content_type: "text/html; charset=utf-8".to_string(),
        })
    } else {
        let content_type = if mime.type_() == mime_guess::mime::TEXT
            || mime.essence_str() == "application/javascript"
        {
            format!("{}; charset=utf-8", mime.essence_str())
        } else {
            mime.essence_str().to_string()
        };
        let file = tokio::fs::File::open(&path).await.map_err(io_err)?;
        Ok(ArtifactFile {
            body: ArtifactBody::File(file),
            content_type,
        })
    }
}

/// Resolve `rest` inside a folder artifact, or `None` if it names nothing there.
async fn resolve_in_folder(root: &Path, rest: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    let wants_index = rest.is_empty() || rest.ends_with('/');
    for segment in rest.split('/').filter(|s| !s.is_empty()) {
        // Only `.`/`..` (traversal) and a literal backslash are refused. A
        // dot-prefixed filename like `.well-known/` or `.env.example` is an
        // ordinary file the artifact's author chose to include, and is
        // otherwise served like any other; the canonicalize check below is
        // the actual traversal guard, this just avoids hitting the
        // filesystem for the common case.
        if segment == "." || segment == ".." || segment.contains('\\') {
            return None;
        }
        path.push(segment);
    }
    if wants_index {
        path.push("index.html");
    }

    let canonical_root = tokio::fs::canonicalize(root).await.ok()?;
    let canonical = tokio::fs::canonicalize(&path).await.ok()?;
    canonical.starts_with(&canonical_root).then_some(canonical)
}

/// Why an artifact could not be deleted.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ArtifactDeleteError {
    #[error("invalid artifact name {0:?}: use lowercase letters, digits, and single hyphens")]
    InvalidName(String),
    #[error("workbench artifact {0:?} does not exist")]
    NotFound(String),
    #[error("failed to delete workbench artifact {name:?}: {source}")]
    Io {
        name: String,
        #[source]
        source: std::io::Error,
    },
}

/// Delete an artifact (its page, or its whole folder) and its data files (regular
/// files named `<name>.*`). Returns the entries removed.
///
/// # Errors
/// Returns [`ArtifactDeleteError`] if the name is invalid, the artifact does not
/// exist, or something cannot be removed. Entries removed before a failure
/// stay removed.
pub(crate) async fn delete_artifact(
    dir: &Path,
    name: &str,
) -> Result<Vec<String>, ArtifactDeleteError> {
    if !is_valid_artifact_name(name) {
        return Err(ArtifactDeleteError::InvalidName(name.to_string()));
    }
    let io_err = |source| ArtifactDeleteError::Io {
        name: name.to_string(),
        source,
    };

    let folder = dir.join(name);
    let has_folder = tokio::fs::symlink_metadata(&folder)
        .await
        .is_ok_and(|m| m.is_dir());
    let has_page = tokio::fs::symlink_metadata(dir.join(format!("{name}.html")))
        .await
        .is_ok_and(|m| m.is_file());
    if !has_folder && !has_page {
        return Err(ArtifactDeleteError::NotFound(name.to_string()));
    }

    let mut removed = Vec::new();
    if has_folder {
        tokio::fs::remove_dir_all(&folder).await.map_err(io_err)?;
        removed.push(format!("{name}/"));
    }

    let prefix = format!("{name}.");
    let mut read_dir = tokio::fs::read_dir(dir).await.map_err(io_err)?;
    while let Some(entry) = read_dir.next_entry().await.map_err(io_err)? {
        let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !file_name.starts_with(&prefix) {
            continue;
        }
        let file_type = entry.file_type().await.map_err(io_err)?;
        if file_type.is_dir() {
            continue;
        }
        tokio::fs::remove_file(entry.path()).await.map_err(io_err)?;
        removed.push(file_name);
    }
    removed.sort();
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_names() {
        for good in ["a", "pricing-explorer", "sdlc-pipeline-v2", "x9"] {
            assert!(is_valid_artifact_name(good), "{good} should be valid");
        }
        for bad in [
            "",
            "Pricing",
            "-lead",
            "trail-",
            "double--hyphen",
            "has.dot",
            "../escape",
            "a/b",
            "under_score",
            "space name",
            &"a".repeat(65),
        ] {
            assert!(!is_valid_artifact_name(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn extract_title_variants() {
        assert_eq!(
            extract_title("<html><head><title>Pricing  Explorer</title></head>").as_deref(),
            Some("Pricing Explorer")
        );
        assert_eq!(
            extract_title("<TITLE lang=\"en\">\n  R&amp;D &lt;3\n</TITLE>").as_deref(),
            Some("R&D <3")
        );
        assert_eq!(extract_title("<title>   </title>"), None);
        assert_eq!(extract_title("<p>no title</p>"), None);
        assert_eq!(extract_title("<title>unterminated"), None);
        assert_eq!(
            extract_title("<titlebar>x</titlebar><title>Real</title>").as_deref(),
            Some("Real")
        );
    }

    #[test]
    fn inject_sdk_goes_after_head_open_tag() {
        let out = inject_sdk(
            "<!doctype html><html><head lang=x><title>t</title></head></html>",
            "chart",
            "2026.09.23",
            &[],
        );
        let script_at = out.find("<script>").unwrap();
        assert_eq!(
            out.get(..script_at).unwrap(),
            "<!doctype html><html><head lang=x>"
        );
        assert!(out.contains("window.residuum"));
        assert!(out.ends_with("<title>t</title></head></html>"));
    }

    #[test]
    fn inject_sdk_skips_header_element_and_falls_back_to_html() {
        let out = inject_sdk(
            "<html><body><header>h</header></body></html>",
            "chart",
            "2026.09.23",
            &[],
        );
        assert!(
            out.starts_with("<html><script>"),
            "got {}",
            out.get(..40).unwrap()
        );
    }

    #[test]
    fn inject_sdk_prepends_to_fragments() {
        let out = inject_sdk("<div>fragment</div>", "chart", "2026.09.23", &[]);
        assert!(out.starts_with("<script>"));
        assert!(out.ends_with("<div>fragment</div>"));
    }

    #[test]
    fn inject_sdk_embeds_artifact_name_version_and_features() {
        let out = inject_sdk(
            "<html></html>",
            "pricing-explorer",
            "2026.09.23",
            &["workspace-tree", "model-complete"],
        );
        assert!(out.contains(r#"<script>{const __RESIDUUM_ARTIFACT__="pricing-explorer";"#));
        // Windows checkouts may give the SDK source CRLF line endings.
        assert!(
            out.replace("\r\n", "\n").contains("})();\n}</script>"),
            "context constants stay block-scoped to the SDK"
        );
        assert!(out.contains(r#"const __RESIDUUM_VERSION__="2026.09.23";"#));
        assert!(
            out.contains(r#"const __RESIDUUM_FEATURES__=["workspace-tree","model-complete"];"#)
        );
    }

    #[test]
    fn inject_sdk_escapes_embedded_values_against_closing_the_script_tag() {
        let out = inject_sdk("<html></html>", "</script><script>evil</script>", "1", &[]);
        assert!(!out.to_ascii_lowercase().contains("</script><script>evil"));
        assert!(out.contains(r"<\/script>"));
    }

    #[test]
    fn sdk_cannot_close_its_own_script_tag() {
        assert!(!SDK_JS.to_ascii_lowercase().contains("</script"));
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn list_artifacts_covers_pages_and_folders_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(&p.join("older.html"), "<title>Older</title>");
        write(&p.join("older.state.json"), "{}");
        write(&p.join("Bad Name.html"), "x");
        write(&p.join("no-index/app.js"), "x");
        let old = SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(p.join("older.html"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        write(&p.join("graph/index.html"), "<title>Wiki Graph</title>");
        write(&p.join("graph/lib/app.js"), "x");

        let artifacts = list_artifacts(p).await.unwrap();
        let names: Vec<_> = artifacts.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["graph", "older"]);
        let titles: Vec<_> = artifacts.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Wiki Graph", "Older"]);
    }

    #[tokio::test]
    async fn folder_wins_over_page_with_the_same_name() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("chart.html"), "<title>Page</title>");
        write(
            &dir.path().join("chart/index.html"),
            "<title>Folder</title>",
        );
        let artifacts = discover_artifacts(dir.path()).await.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert!(matches!(
            artifacts.first().map(|t| &t.kind),
            Some(ArtifactKind::Folder(_))
        ));
    }

    #[tokio::test]
    async fn list_artifacts_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            list_artifacts(&dir.path().join("nope"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    async fn body_to_bytes(body: ArtifactBody) -> Vec<u8> {
        match body {
            ArtifactBody::Bytes(b) => b,
            ArtifactBody::File(mut f) => {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf).await.unwrap();
                buf
            }
        }
    }

    #[tokio::test]
    async fn reads_pages_and_folder_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(&p.join("single.html"), "<head></head>");
        write(&p.join("graph/index.html"), "<head></head>");
        write(&p.join("graph/lib/app.js"), "console.log(1)");
        write(&p.join("graph/docs/index.html"), "<p>docs</p>");

        let page = read_artifact_file(p, "single", "").await.unwrap();
        assert!(
            String::from_utf8(body_to_bytes(page.body).await)
                .unwrap()
                .starts_with("<head><script>")
        );
        assert_eq!(page.content_type, "text/html; charset=utf-8");

        let index = read_artifact_file(p, "graph", "").await.unwrap();
        assert!(
            String::from_utf8(body_to_bytes(index.body).await)
                .unwrap()
                .contains("window.residuum")
        );

        let script = read_artifact_file(p, "graph", "lib/app.js").await.unwrap();
        assert_eq!(body_to_bytes(script.body).await, b"console.log(1)");
        assert!(script.content_type.contains("javascript"));

        let nested = read_artifact_file(p, "graph", "docs/").await.unwrap();
        assert!(
            String::from_utf8(body_to_bytes(nested.body).await)
                .unwrap()
                .contains("window.residuum")
        );

        assert!(matches!(
            read_artifact_file(p, "single", "other.js").await,
            Err(ArtifactFileError::NotFound { .. })
        ));
        assert!(matches!(
            read_artifact_file(p, "missing", "").await,
            Err(ArtifactFileError::NoSuchArtifact(_))
        ));
    }

    #[tokio::test]
    async fn a_file_over_the_old_eight_mb_cap_is_streamed_not_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(&p.join("big/index.html"), "<head></head>");
        let big = vec![b'x'; 9 * 1024 * 1024];
        std::fs::write(p.join("big/data.bin"), &big).unwrap();

        let served = read_artifact_file(p, "big", "data.bin").await.unwrap();
        assert!(matches!(served.body, ArtifactBody::File(_)));
        assert_eq!(body_to_bytes(served.body).await.len(), big.len());
    }

    #[tokio::test]
    async fn folder_paths_cannot_escape_the_artifact_root() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(&p.join("graph/index.html"), "x");
        write(&p.join("other/index.html"), "x");
        write(&p.join("graph.state.json"), "{}");

        for rest in ["../other/index.html", "..", "a/../../graph.state.json"] {
            assert!(
                matches!(
                    read_artifact_file(p, "graph", rest).await,
                    Err(ArtifactFileError::NotFound { .. })
                ),
                "{rest} must not resolve"
            );
        }
    }

    #[tokio::test]
    async fn folder_paths_allow_dot_prefixed_files() {
        // A dot-prefixed segment (e.g. a build tool's `.well-known/` or an
        // author's `.env.example`) is an ordinary file within the artifact's
        // own folder, not a traversal attempt — it should serve normally.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(&p.join("graph/index.html"), "x");
        write(&p.join("graph/.secret"), "shh");

        let served = read_artifact_file(p, "graph", ".secret").await.unwrap();
        assert_eq!(body_to_bytes(served.body).await, b"shh");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_cannot_expose_outside_files() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.html");
        std::fs::write(&secret, "<title>Secret</title>").unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join("leak.html")).unwrap();
        write(&dir.path().join("graph/index.html"), "x");
        std::os::unix::fs::symlink(&secret, dir.path().join("graph/leak.html")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();

        let names: Vec<_> = list_artifacts(dir.path())
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["graph"]);
        assert!(read_artifact_file(dir.path(), "leak", "").await.is_err());
        assert!(
            read_artifact_file(dir.path(), "graph", "leak.html")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn delete_artifact_removes_page_and_data_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(&p.join("chart.html"), "x");
        write(&p.join("chart.state.json"), "{}");
        write(&p.join("chart-two.html"), "x");
        write(&p.join("chartx.json"), "{}");

        let removed = delete_artifact(p, "chart").await.unwrap();
        assert_eq!(removed, ["chart.html", "chart.state.json"]);
        assert!(p.join("chart-two.html").exists());
        assert!(p.join("chartx.json").exists());

        assert!(matches!(
            delete_artifact(p, "chart").await,
            Err(ArtifactDeleteError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn delete_artifact_removes_a_folder_and_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(&p.join("graph/index.html"), "x");
        write(&p.join("graph/lib/app.js"), "x");
        write(&p.join("graph.state.json"), "{}");

        let removed = delete_artifact(p, "graph").await.unwrap();
        assert_eq!(removed, ["graph.state.json", "graph/"]);
        assert!(!p.join("graph").exists());
    }
}
