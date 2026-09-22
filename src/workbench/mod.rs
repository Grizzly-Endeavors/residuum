//! Workbench: single-file HTML tools the agent builds for the user.
//!
//! A tool is `<workspace>/workbench/<name>.html`, where `<name>` is kebab-case.
//! The web UI lists tools at `/workbench` and shows one at `/workbench/<name>`
//! inside a sandboxed frame. Files beside a tool that share its `<name>.`
//! prefix (for example `<name>.state.json`) are that tool's data: they are not
//! listed as tools and are deleted with it.

pub(crate) mod watcher;

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::gateway::protocol::WorkbenchToolSummary;

/// Tool pages larger than this are refused rather than served.
pub(crate) const MAX_TOOL_PAGE_BYTES: u64 = 8 * 1024 * 1024;

/// Only the start of a page is scanned for its `<title>` when listing.
const TITLE_SCAN_BYTES: usize = 64 * 1024;

const MAX_TOOL_NAME_LEN: usize = 64;

const TOOL_EXTENSION: &str = "html";

/// The SDK injected into every served tool page.
const SDK_JS: &str = include_str!("../../assets/workbench/sdk.js");

/// Why a tool page could not be read.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolPageError {
    #[error("invalid tool name {0:?}: use lowercase letters, digits, and single hyphens")]
    InvalidName(String),
    #[error("workbench tool {0:?} does not exist")]
    NotFound(String),
    #[error("workbench tool {name:?} is {size} bytes, over the {MAX_TOOL_PAGE_BYTES}-byte limit")]
    TooLarge { name: String, size: u64 },
    #[error("failed to read workbench tool {name:?}: {source}")]
    Io {
        name: String,
        #[source]
        source: std::io::Error,
    },
}

/// Whether `name` is a valid tool name: lowercase ASCII letters and digits in
/// hyphen-separated words, at most 64 characters. Names carry no path
/// separators or dots, so a valid name always resolves inside the workbench.
#[must_use]
pub(crate) fn is_valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOOL_NAME_LEN
        && name.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

fn tool_page_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.{TOOL_EXTENSION}"))
}

/// The tool name a directory entry's file name denotes, if it is a tool page.
fn tool_name_of(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".html")?;
    is_valid_tool_name(stem).then_some(stem)
}

/// List the tools in `dir`, most recently modified first.
///
/// A missing directory is an empty workbench. Symlinks are skipped so a tool
/// can never expose a file from outside the workbench.
///
/// # Errors
/// Returns an error if the directory exists but cannot be read.
pub(crate) async fn list_tools(dir: &Path) -> std::io::Result<Vec<WorkbenchToolSummary>> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut tools = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str().and_then(tool_name_of) else {
            continue;
        };
        let file_type = entry.file_type().await?;
        if !file_type.is_file() {
            continue;
        }
        let metadata = entry.metadata().await?;
        let title = match read_title(&entry.path()).await {
            Ok(title) => title,
            Err(e) => {
                tracing::warn!(tool = name, error = %e, "failed to read workbench tool title");
                None
            }
        };
        tools.push(WorkbenchToolSummary {
            name: name.to_string(),
            title: title.unwrap_or_else(|| name.to_string()),
            modified_at: metadata
                .modified()
                .map_or_else(|_| DateTime::<Utc>::UNIX_EPOCH, to_utc),
            size: metadata.len(),
        });
    }

    tools.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(tools)
}

fn to_utc(time: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(time)
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

/// Read a tool page and inject the workbench SDK into it.
///
/// # Errors
/// Returns [`ToolPageError`] if the name is invalid, the page does not exist
/// (or is not a regular file), is over [`MAX_TOOL_PAGE_BYTES`], or cannot be
/// read.
pub(crate) async fn read_tool_page(dir: &Path, name: &str) -> Result<String, ToolPageError> {
    if !is_valid_tool_name(name) {
        return Err(ToolPageError::InvalidName(name.to_string()));
    }
    let path = tool_page_path(dir, name);
    let io_err = |source| ToolPageError::Io {
        name: name.to_string(),
        source,
    };

    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(ToolPageError::NotFound(name.to_string()));
        }
        Err(e) => return Err(io_err(e)),
    };
    if !metadata.is_file() {
        return Err(ToolPageError::NotFound(name.to_string()));
    }
    if metadata.len() > MAX_TOOL_PAGE_BYTES {
        return Err(ToolPageError::TooLarge {
            name: name.to_string(),
            size: metadata.len(),
        });
    }

    let bytes = tokio::fs::read(&path).await.map_err(io_err)?;
    Ok(inject_sdk(&String::from_utf8_lossy(&bytes)))
}

/// Insert the SDK `<script>` so it runs before any of the page's own scripts:
/// right after the `<head>` open tag, else after `<html>`, else after the
/// doctype, else at the very start.
#[must_use]
pub(crate) fn inject_sdk(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let insert_at = ["head", "html", "!doctype"]
        .iter()
        .find_map(|tag| {
            let open = find_tag(&lower, tag, 0)?;
            Some(open + lower.get(open..)?.find('>')? + 1)
        })
        .unwrap_or(0);

    let script = format!("<script>{SDK_JS}</script>");
    let mut out = String::with_capacity(html.len() + script.len());
    out.push_str(html.get(..insert_at).unwrap_or_default());
    out.push_str(&script);
    out.push_str(html.get(insert_at..).unwrap_or(html));
    out
}

/// Why a tool could not be deleted.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolDeleteError {
    #[error("invalid tool name {0:?}: use lowercase letters, digits, and single hyphens")]
    InvalidName(String),
    #[error("workbench tool {0:?} does not exist")]
    NotFound(String),
    #[error("failed to delete workbench tool {name:?}: {source}")]
    Io {
        name: String,
        #[source]
        source: std::io::Error,
    },
}

/// Delete a tool's page and its data files (regular files named `<name>.*`).
/// Returns the file names removed.
///
/// # Errors
/// Returns [`ToolDeleteError`] if the name is invalid, the tool has no page,
/// or a file cannot be removed. Files removed before a failure stay removed.
pub(crate) async fn delete_tool(dir: &Path, name: &str) -> Result<Vec<String>, ToolDeleteError> {
    if !is_valid_tool_name(name) {
        return Err(ToolDeleteError::InvalidName(name.to_string()));
    }
    let io_err = |source| ToolDeleteError::Io {
        name: name.to_string(),
        source,
    };

    match tokio::fs::symlink_metadata(tool_page_path(dir, name)).await {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(ToolDeleteError::NotFound(name.to_string())),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(ToolDeleteError::NotFound(name.to_string()));
        }
        Err(e) => return Err(io_err(e)),
    }

    let prefix = format!("{name}.");
    let mut removed = Vec::new();
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
    fn tool_names() {
        for good in ["a", "pricing-explorer", "sdlc-pipeline-v2", "x9"] {
            assert!(is_valid_tool_name(good), "{good} should be valid");
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
            assert!(!is_valid_tool_name(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn tool_name_of_requires_html_and_valid_stem() {
        assert_eq!(tool_name_of("chart.html"), Some("chart"));
        assert_eq!(tool_name_of("chart.state.json"), None);
        assert_eq!(tool_name_of("chart.data.html"), None);
        assert_eq!(tool_name_of("Chart.html"), None);
        assert_eq!(tool_name_of("chart.htm"), None);
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
        let out = inject_sdk("<!doctype html><html><head lang=x><title>t</title></head></html>");
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
        let out = inject_sdk("<html><body><header>h</header></body></html>");
        assert!(
            out.starts_with("<html><script>"),
            "got {}",
            out.get(..40).unwrap()
        );
    }

    #[test]
    fn inject_sdk_prepends_to_fragments() {
        let out = inject_sdk("<div>fragment</div>");
        assert!(out.starts_with("<script>"));
        assert!(out.ends_with("<div>fragment</div>"));
    }

    #[test]
    fn sdk_cannot_close_its_own_script_tag() {
        assert!(!SDK_JS.to_ascii_lowercase().contains("</script"));
    }

    #[tokio::test]
    async fn list_tools_skips_non_tools_and_sorts_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("older.html"), "<title>Older</title>").unwrap();
        std::fs::write(p.join("older.state.json"), "{}").unwrap();
        std::fs::write(p.join("Bad Name.html"), "x").unwrap();
        std::fs::create_dir(p.join("folder.html")).unwrap();
        let old = SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(p.join("older.html"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::write(p.join("newer.html"), "<p>untitled</p>").unwrap();

        let tools = list_tools(p).await.unwrap();
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["newer", "older"]);
        let titles: Vec<_> = tools.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            titles,
            ["newer", "Older"],
            "untitled tools fall back to the name"
        );
    }

    #[tokio::test]
    async fn list_tools_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            list_tools(&dir.path().join("nope"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_tools_are_neither_listed_nor_served() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.html");
        std::fs::write(&secret, "<title>Secret</title>").unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join("leak.html")).unwrap();

        assert!(list_tools(dir.path()).await.unwrap().is_empty());
        assert!(matches!(
            read_tool_page(dir.path(), "leak").await,
            Err(ToolPageError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn read_tool_page_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_tool_page(dir.path(), "../etc").await,
            Err(ToolPageError::InvalidName(_))
        ));
        assert!(matches!(
            read_tool_page(dir.path(), "missing").await,
            Err(ToolPageError::NotFound(_))
        ));
        std::fs::write(dir.path().join("ok.html"), "<head></head>").unwrap();
        let page = read_tool_page(dir.path(), "ok").await.unwrap();
        assert!(page.starts_with("<head><script>"));
    }

    #[tokio::test]
    async fn delete_tool_removes_page_and_data_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("chart.html"), "x").unwrap();
        std::fs::write(p.join("chart.state.json"), "{}").unwrap();
        std::fs::write(p.join("chart-two.html"), "x").unwrap();
        std::fs::write(p.join("chartx.json"), "{}").unwrap();

        let removed = delete_tool(p, "chart").await.unwrap();
        assert_eq!(removed, ["chart.html", "chart.state.json"]);
        assert!(p.join("chart-two.html").exists());
        assert!(p.join("chartx.json").exists());

        assert!(matches!(
            delete_tool(p, "chart").await,
            Err(ToolDeleteError::NotFound(_))
        ));
    }
}
