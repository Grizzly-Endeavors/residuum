//! Inbox system: a general-purpose "deal with it later" queue.
//!
//! Items are stored as individual JSON files in the workspace `inbox/` directory.
//! External systems can add items by dropping `.json` files into the directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::interfaces::attachment::FileAttachment;

/// Workspace-root-relative directory that user inbox attachments are copied into,
/// one subdirectory per item ID. Mirrors `WorkspaceLayout::user_inbox_attachments_dir`
/// as a string literal because this module doesn't depend on `workspace::layout`.
const USER_INBOX_ATTACHMENTS_REL: &str = "inbox/user/attachments";

/// A single inbox item stored as a JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxItem {
    /// Short summary of the item.
    pub title: String,
    /// Full body text.
    pub body: String,
    /// Origin label (e.g. `"cron:backup"`, `"discord"`, `"agent"`).
    pub source: String,
    /// When the item was created.
    #[serde(with = "crate::time::minute_format")]
    pub timestamp: NaiveDateTime,
    /// Whether the agent has read this item.
    pub read: bool,
    /// Paths to related files, relative to the workspace root at the time they were
    /// attached. Consumers should treat only the final path component (the filename)
    /// as meaningful — see `copy_attachments` for how these are populated and
    /// `archive_item` for why the directory portion can go stale.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PathBuf>,
}

/// Generate a filename for an inbox item: `{YYYYMMDD}_{sanitized_title}.json`.
///
/// Sanitizes: lowercase, non-alphanumeric → `_`, collapse consecutive `_`, truncate to 60 chars.
#[must_use]
pub fn generate_filename(title: &str, now: NaiveDateTime) -> String {
    let date = now.format("%Y%m%d").to_string();

    let sanitized: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();

    let slug = sanitized
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    let truncated = slug.chars().take(60).collect::<String>();
    let truncated = truncated.trim_end_matches('_');

    if truncated.is_empty() {
        format!("{date}.json")
    } else {
        format!("{date}_{truncated}.json")
    }
}

/// Default an inbox item's title from its body: the first line, cut to 60
/// characters. Shared by every caller that lets a title be omitted — the WS
/// `/inbox` command and the `POST /api/agent-inbox` endpoint — so the rule
/// stays in one place.
#[must_use]
pub fn derive_title(body: &str) -> String {
    body.lines()
        .next()
        .unwrap_or("Inbox message")
        .chars()
        .take(60)
        .collect()
}

/// Add an inbox item in one call: generates a filename, builds the item, and saves.
///
/// Returns the filename for confirmation messages.
///
/// # Errors
/// Returns an error if the item cannot be saved.
#[tracing::instrument(skip_all, fields(source = %source))]
pub async fn quick_add(
    inbox_dir: &Path,
    title: &str,
    body: &str,
    source: &str,
    tz: chrono_tz::Tz,
) -> anyhow::Result<String> {
    let now = crate::time::now_local(tz);
    let filename = generate_filename(title, now);
    let item = InboxItem {
        title: title.to_string(),
        body: body.to_string(),
        source: source.to_string(),
        timestamp: now,
        read: false,
        attachments: Vec::new(),
    };
    save_item(inbox_dir, &filename, &item).await?;
    tracing::debug!(filename = %filename, title = %title, "inbox item created via quick_add");
    Ok(filename)
}

/// Add a user inbox item with attachments in one call: copies the attachment files
/// into the item's own directory, generates a filename, builds the item, and saves.
///
/// Behaves exactly like `quick_add` when `attachment_paths` is empty. Otherwise, see
/// `copy_attachments` for how the files are copied and named. The item is saved with
/// whichever attachments succeeded even if some failed; the second return value
/// describes each failure so the caller can tell the user about it.
///
/// Returns the filename for confirmation messages, and a description of each
/// attachment that failed to copy (empty if all succeeded).
///
/// # Errors
/// Returns an error if the item cannot be saved, or an attachment directory
/// cannot be created.
#[tracing::instrument(skip_all, fields(source = %source))]
pub async fn quick_add_with_attachments(
    inbox_dir: &Path,
    attachments_dir: &Path,
    title: &str,
    body: &str,
    source: &str,
    tz: chrono_tz::Tz,
    attachment_paths: &[PathBuf],
) -> anyhow::Result<(String, Vec<String>)> {
    let now = crate::time::now_local(tz);
    let filename = generate_filename(title, now);
    let item_id = filename.trim_end_matches(".json");

    let (attachments, failures) =
        copy_attachments(attachments_dir, item_id, attachment_paths).await?;

    let item = InboxItem {
        title: title.to_string(),
        body: body.to_string(),
        source: source.to_string(),
        timestamp: now,
        read: false,
        attachments,
    };
    save_item(inbox_dir, &filename, &item).await?;
    tracing::debug!(
        filename = %filename,
        title = %title,
        attachment_count = attachment_paths.len(),
        failure_count = failures.len(),
        "inbox item created via quick_add_with_attachments"
    );
    Ok((filename, failures))
}

/// Copy attachment source files into an item's own attachment directory
/// (`attachments_dir/<item_id>/`), keyed by item ID so the item survives its
/// sources being moved or deleted.
///
/// Each source path is validated (must exist and be readable — there is no
/// size cap, since these are already-local files, not something arriving
/// over a platform with its own upload limit) before being copied. Only the
/// source path's final component is used as the destination filename, so a
/// traversal-style source path (e.g. `../../../etc/passwd`) cannot place a copy
/// outside the item's directory; collisions within the batch are resolved by
/// appending a numeric suffix rather than overwriting.
///
/// A source that fails validation or copy is skipped, not fatal to the whole
/// batch: the item still gets every attachment that did succeed. Returns the
/// successfully-copied attachments plus a description of each one that
/// failed, so the caller can tell the user what happened to each file.
///
/// # Errors
/// Returns an error only if the item's attachment directory itself cannot be
/// created — a per-file failure is reported in the second return value
/// instead.
#[tracing::instrument(skip_all, fields(item_id = %item_id, count = source_paths.len()))]
pub async fn copy_attachments(
    attachments_dir: &Path,
    item_id: &str,
    source_paths: &[PathBuf],
) -> anyhow::Result<(Vec<PathBuf>, Vec<String>)> {
    use anyhow::Context as _;

    if source_paths.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let item_dir = attachments_dir.join(item_id);
    tokio::fs::create_dir_all(&item_dir)
        .await
        .with_context(|| format!("failed to create attachment dir {}", item_dir.display()))?;

    let mut used_names: HashSet<String> = HashSet::new();
    let mut recorded = Vec::new();
    let mut failures = Vec::new();

    for source in source_paths {
        match copy_one_attachment(source, &item_dir, &mut used_names).await {
            Ok(unique_name) => {
                recorded.push(
                    PathBuf::from(USER_INBOX_ATTACHMENTS_REL)
                        .join(item_id)
                        .join(unique_name),
                );
            }
            Err(e) => {
                tracing::warn!(
                    item_id = %item_id,
                    source = %source.display(),
                    error = %e,
                    "failed to copy one inbox attachment; continuing with the rest"
                );
                failures.push(format!("couldn't attach '{}': {e}", source.display()));
            }
        }
    }

    Ok((recorded, failures))
}

/// Validate and copy a single attachment source into `item_dir`, returning the
/// unique destination filename that was used.
async fn copy_one_attachment(
    source: &Path,
    item_dir: &Path,
    used_names: &mut HashSet<String>,
) -> anyhow::Result<String> {
    use anyhow::Context as _;

    let attachment = FileAttachment::from_path(source)
        .await
        .map_err(anyhow::Error::msg)?;

    let sanitized = sanitize_attachment_filename(&attachment.filename);
    let unique = dedupe_filename(&sanitized, used_names);
    let dest = item_dir.join(&unique);

    tokio::fs::copy(source, &dest).await.with_context(|| {
        format!(
            "failed to copy attachment from {} to {}",
            source.display(),
            dest.display()
        )
    })?;

    used_names.insert(unique.clone());
    Ok(unique)
}

/// Reduce an attachment's filename to a safe, storable name.
///
/// `filename` is already just a file name (callers derive it via `Path::file_name`,
/// which strips any directory components), but it can still contain characters that
/// don't belong in a stored filename, or collapse to nothing meaningful once those
/// are removed.
fn sanitize_attachment_filename(filename: &str) -> String {
    let cleaned: String = filename
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0'))
        .collect();
    match cleaned.trim() {
        "" | "." | ".." => "attachment".to_string(),
        name => name.to_string(),
    }
}

/// Return a filename guaranteed not to be in `used`, appending `_2`, `_3`, etc.
/// before the extension when `name` collides with one already used.
fn dedupe_filename(name: &str, used: &HashSet<String>) -> String {
    if !used.contains(name) {
        return name.to_string();
    }

    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string();
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_string);

    let mut n: u32 = 2;
    loop {
        let candidate = match &ext {
            Some(ext) => format!("{stem}_{n}.{ext}"),
            None => format!("{stem}_{n}"),
        };
        if !used.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Save an inbox item atomically (write to `.tmp`, then rename).
///
/// # Errors
/// Returns an error if serialization or file operations fail.
#[tracing::instrument(skip_all, fields(filename = %filename))]
pub async fn save_item(inbox_dir: &Path, filename: &str, item: &InboxItem) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let target = inbox_dir.join(filename);

    let json = serde_json::to_string_pretty(item)
        .with_context(|| format!("failed to serialize inbox item for {}", target.display()))?;

    crate::util::fs::atomic_write(&target, &json)
        .await
        .inspect_err(|e| {
            tracing::warn!(path = %target.display(), error = %e, "atomic write failed, orphaned tmp may remain");
        })?;

    Ok(())
}

/// Load a single inbox item from a JSON file.
///
/// # Errors
/// Returns an error if the file cannot be read or parsed.
#[tracing::instrument(skip_all, fields(path = %path.display()))]
pub async fn load_item(path: &Path) -> anyhow::Result<InboxItem> {
    use anyhow::Context as _;

    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read inbox item at {}", path.display()))?;
    let item: InboxItem = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse inbox item at {}", path.display()))?;
    Ok(item)
}

/// List all inbox items (non-recursive, ignores subdirectories).
///
/// Returns `(filename_stem, item)` pairs sorted newest-first by timestamp.
///
/// # Errors
/// Returns an error if the directory cannot be read.
#[tracing::instrument(skip_all, fields(path = %inbox_dir.display()))]
pub async fn list_items(inbox_dir: &Path) -> anyhow::Result<Vec<(String, InboxItem)>> {
    use anyhow::Context as _;

    let mut entries = Vec::new();
    let mut dir = tokio::fs::read_dir(inbox_dir)
        .await
        .with_context(|| format!("failed to read inbox directory {}", inbox_dir.display()))?;

    while let Some(entry) = dir.next_entry().await? {
        let ftype = entry.file_type().await?;
        if !ftype.is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("json") {
            continue;
        }

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        match load_item(&path).await {
            Ok(item) => entries.push((stem, item)),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping malformed inbox item");
            }
        }
    }

    entries.sort_by_key(|b| std::cmp::Reverse(b.1.timestamp));
    Ok(entries)
}

/// Count unread inbox items.
#[tracing::instrument(skip_all, fields(path = %inbox_dir.display()))]
pub async fn count_unread(inbox_dir: &Path) -> usize {
    match list_items(inbox_dir).await {
        Ok(items) => items.iter().filter(|(_, i)| !i.read).count(),
        Err(e) => {
            tracing::warn!(path = %inbox_dir.display(), error = %e, "failed to list inbox items for unread count");
            0
        }
    }
}

/// Mark an inbox item as read and save it back atomically.
///
/// # Errors
/// Returns an error if the file cannot be found, read, or written.
#[tracing::instrument(skip_all, fields(filename = %filename))]
pub async fn mark_read(inbox_dir: &Path, filename: &str) -> anyhow::Result<InboxItem> {
    use anyhow::Context as _;

    let json_name = ensure_json_ext(filename);
    let path = inbox_dir.join(&json_name);

    let mut item = load_item(&path)
        .await
        .with_context(|| format!("failed to load inbox item {json_name} for mark_read"))?;
    item.read = true;
    save_item(inbox_dir, &json_name, &item)
        .await
        .with_context(|| format!("failed to save inbox item {json_name} after mark_read"))?;
    tracing::debug!(filename = %json_name, "inbox item marked read");

    Ok(item)
}

/// Move an inbox item to the inbox archive directory (`archive/inbox/`).
///
/// If the item has an `attachments/<item_id>/` subdirectory sitting alongside its
/// JSON file — which only the user inbox ever populates — that directory moves
/// into the archive alongside it, so an archived item's attachments keep serving.
/// This is a no-op for items with no such directory (every agent-inbox item, and
/// any user-inbox item with no attachments).
///
/// The attachment directory is moved before the JSON file: if the attachment move
/// fails, the item is left untouched and archiving can be retried; if the JSON move
/// then fails, retrying is still safe since the attachment move finds nothing left
/// to move.
///
/// # Errors
/// Returns an error if the file is not found, or if either move fails.
#[tracing::instrument(skip_all, fields(item = %filename))]
pub async fn archive_item(
    inbox_dir: &Path,
    archive_dir: &Path,
    filename: &str,
) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let json_name = ensure_json_ext(filename);
    let item_id = json_name.trim_end_matches(".json").to_string();
    let src = inbox_dir.join(&json_name);

    tokio::fs::create_dir_all(archive_dir)
        .await
        .with_context(|| format!("failed to create archive dir {}", archive_dir.display()))?;

    let src_attachments = inbox_dir.join("attachments").join(&item_id);
    if tokio::fs::try_exists(&src_attachments)
        .await
        .unwrap_or(false)
    {
        let dst_attachments_root = archive_dir.join("attachments");
        tokio::fs::create_dir_all(&dst_attachments_root)
            .await
            .with_context(|| {
                format!(
                    "failed to create archive attachments dir {}",
                    dst_attachments_root.display()
                )
            })?;
        let dst_attachments = dst_attachments_root.join(&item_id);
        tokio::fs::rename(&src_attachments, &dst_attachments)
            .await
            .with_context(|| format!("failed to archive attachments for inbox item '{item_id}'"))?;
        tracing::debug!(
            src = %src_attachments.display(),
            dst = %dst_attachments.display(),
            "inbox item attachments archived"
        );
    }

    let dst = archive_dir.join(&json_name);
    tokio::fs::rename(&src, &dst)
        .await
        .with_context(|| format!("inbox item '{json_name}' not found or could not be archived"))?;
    tracing::debug!(src = %src.display(), dst = %dst.display(), "inbox item archived");

    Ok(())
}

/// Ensure a filename ends with `.json`.
fn ensure_json_ext(name: &str) -> String {
    if Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        name.to_string()
    } else {
        format!("{name}.json")
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    clippy::string_slice,
    reason = "test code uses indexing and string slicing for clarity"
)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn test_now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 2, 25)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn make_item(title: &str, hour: u32, read: bool) -> InboxItem {
        InboxItem {
            title: title.to_string(),
            body: format!("Body for {title}"),
            source: "test".to_string(),
            timestamp: NaiveDate::from_ymd_opt(2026, 2, 25)
                .unwrap()
                .and_hms_opt(hour, 0, 0)
                .unwrap(),
            read,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn derive_title_takes_first_line() {
        assert_eq!(derive_title("Hello world\nmore body text"), "Hello world");
    }

    #[test]
    fn derive_title_truncates_to_60_chars() {
        let long_line = "a".repeat(100);
        assert_eq!(derive_title(&long_line), "a".repeat(60));
    }

    #[test]
    fn derive_title_falls_back_on_empty_body() {
        assert_eq!(derive_title(""), "Inbox message");
    }

    #[test]
    fn generate_filename_basic() {
        let now = test_now();
        assert_eq!(
            generate_filename("Hello World", now),
            "20260225_hello_world.json"
        );
    }

    #[test]
    fn generate_filename_special_chars() {
        let now = test_now();
        let name = generate_filename("foo/bar\\baz..qux!!", now);
        assert!(!name.contains('/'), "should not contain slashes: {name}");
        assert!(
            !name.contains('\\'),
            "should not contain backslashes: {name}"
        );
        assert!(!name.contains("__"), "should collapse underscores: {name}");
    }

    #[test]
    fn generate_filename_unicode() {
        let now = test_now();
        let name = generate_filename("café résumé", now);
        assert_eq!(name, "20260225_café_résumé.json");
    }

    #[test]
    fn generate_filename_empty_title() {
        assert_eq!(generate_filename("", test_now()), "20260225.json");
    }

    #[test]
    fn generate_filename_all_special_chars() {
        assert_eq!(generate_filename("!!!###", test_now()), "20260225.json");
    }

    #[test]
    fn generate_filename_trailing_underscore_trim() {
        let now = test_now();
        let title = "a".repeat(59) + " " + &"b".repeat(40);
        let name = generate_filename(&title, now);
        assert_eq!(name, format!("20260225_{}.json", "a".repeat(59)));
    }

    #[test]
    fn generate_filename_truncation() {
        let now = test_now();
        let long_title = "a".repeat(100);
        let name = generate_filename(&long_title, now);
        // Date prefix (8) + _ (1) + truncated (60) + .json (5) = 74
        let stem = name.trim_end_matches(".json");
        let title_part = &stem["20260225_".len()..];
        assert!(
            title_part.len() <= 60,
            "title part should be at most 60 chars: {} (len={})",
            title_part,
            title_part.len()
        );
    }

    #[tokio::test]
    async fn quick_add_creates_item() {
        let dir = tempfile::tempdir().unwrap();

        let filename = quick_add(dir.path(), "test note", "body text", "cli", chrono_tz::UTC)
            .await
            .unwrap();

        assert!(
            std::path::Path::new(&filename)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json")),
            "should end with .json"
        );
        assert!(
            filename.contains("test_note"),
            "should contain sanitized title: {filename}"
        );

        let item = load_item(&dir.path().join(&filename)).await.unwrap();
        assert_eq!(item.title, "test note");
        assert_eq!(item.body, "body text");
        assert_eq!(item.source, "cli");
        assert!(!item.read);
        assert!(item.attachments.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let item = make_item("test roundtrip", 12, false);

        save_item(dir.path(), "test.json", &item).await.unwrap();
        let loaded = load_item(&dir.path().join("test.json")).await.unwrap();

        assert_eq!(loaded.title, "test roundtrip");
        assert_eq!(loaded.body, "Body for test roundtrip");
        assert_eq!(loaded.source, "test");
        assert!(!loaded.read);
        assert_eq!(loaded.timestamp, item.timestamp);
    }

    #[tokio::test]
    async fn list_items_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let items = list_items(dir.path()).await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn list_items_skips_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let valid = make_item("valid", 12, false);
        save_item(dir.path(), "valid.json", &valid).await.unwrap();
        tokio::fs::write(dir.path().join("bad.json"), b"not json")
            .await
            .unwrap();
        let items = list_items(dir.path()).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].1.title, "valid");
    }

    #[tokio::test]
    async fn list_items_sorted_by_timestamp() {
        let dir = tempfile::tempdir().unwrap();

        let early = make_item("early", 8, false);
        let late = make_item("late", 20, false);
        let mid = make_item("mid", 14, false);

        save_item(dir.path(), "a_early.json", &early).await.unwrap();
        save_item(dir.path(), "b_late.json", &late).await.unwrap();
        save_item(dir.path(), "c_mid.json", &mid).await.unwrap();

        let items = list_items(dir.path()).await.unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].1.title, "late", "newest first");
        assert_eq!(items[1].1.title, "mid");
        assert_eq!(items[2].1.title, "early", "oldest last");
    }

    #[tokio::test]
    async fn list_items_ignores_non_json() {
        let dir = tempfile::tempdir().unwrap();
        let item = make_item("valid", 12, false);
        save_item(dir.path(), "valid.json", &item).await.unwrap();

        // Create non-JSON files
        tokio::fs::write(dir.path().join("photo.png"), b"fake image")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("notes.txt"), b"some notes")
            .await
            .unwrap();

        let items = list_items(dir.path()).await.unwrap();
        assert_eq!(items.len(), 1, "should only include .json files");
    }

    #[tokio::test]
    async fn list_items_ignores_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let item = make_item("active", 12, false);
        save_item(dir.path(), "active.json", &item).await.unwrap();

        // Create archive subdirectory with a json file
        let archive = dir.path().join("archive");
        tokio::fs::create_dir_all(&archive).await.unwrap();
        save_item(&archive, "archived.json", &item).await.unwrap();

        let items = list_items(dir.path()).await.unwrap();
        assert_eq!(items.len(), 1, "should not recurse into archive/");
        assert_eq!(items[0].1.title, "active");
    }

    #[tokio::test]
    async fn list_items_ignores_json_named_dir() {
        let dir = tempfile::tempdir().unwrap();
        let item = make_item("active", 12, false);
        save_item(dir.path(), "active.json", &item).await.unwrap();

        // Create a directory named with a .json extension
        let json_dir = dir.path().join("archive.json");
        tokio::fs::create_dir_all(&json_dir).await.unwrap();

        let items = list_items(dir.path()).await.unwrap();
        assert_eq!(items.len(), 1, "should skip .json-named directories");
        assert_eq!(items[0].1.title, "active");
    }

    #[tokio::test]
    async fn count_unread_accuracy() {
        let dir = tempfile::tempdir().unwrap();

        save_item(dir.path(), "unread1.json", &make_item("a", 12, false))
            .await
            .unwrap();
        save_item(dir.path(), "unread2.json", &make_item("b", 12, false))
            .await
            .unwrap();
        save_item(dir.path(), "read1.json", &make_item("c", 12, true))
            .await
            .unwrap();

        assert_eq!(
            count_unread(dir.path()).await,
            2,
            "should count only unread items"
        );
    }

    #[tokio::test]
    async fn count_unread_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(count_unread(dir.path()).await, 0);
    }

    #[tokio::test]
    async fn count_unread_missing_dir() {
        let missing = Path::new("/tmp/nonexistent_inbox_dir_test");
        assert_eq!(
            count_unread(missing).await,
            0,
            "missing dir should return 0"
        );
    }

    #[tokio::test]
    async fn mark_read_updates_file() {
        let dir = tempfile::tempdir().unwrap();
        save_item(dir.path(), "item.json", &make_item("test", 12, false))
            .await
            .unwrap();

        let item = mark_read(dir.path(), "item").await.unwrap();
        assert!(item.read, "returned item should be marked read");

        // Verify persisted
        let reloaded = load_item(&dir.path().join("item.json")).await.unwrap();
        assert!(reloaded.read, "persisted item should be marked read");
    }

    #[tokio::test]
    async fn archive_item_moves_file() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("inbox");
        let archive = dir.path().join("archive/inbox");
        tokio::fs::create_dir_all(&inbox).await.unwrap();

        save_item(
            &inbox,
            "to_archive.json",
            &make_item("archive me", 12, false),
        )
        .await
        .unwrap();

        archive_item(&inbox, &archive, "to_archive").await.unwrap();

        assert!(
            !inbox.join("to_archive.json").exists(),
            "source should be gone"
        );
        assert!(
            archive.join("to_archive.json").exists(),
            "should be in archive/inbox/"
        );
    }

    #[tokio::test]
    async fn archive_item_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("archive/inbox");
        let result = archive_item(dir.path(), &archive, "nonexistent").await;
        assert!(result.is_err(), "should error on missing file");
    }

    #[tokio::test]
    async fn mark_read_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = mark_read(dir.path(), "nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn attachments_roundtrip_empty() {
        let dir = tempfile::tempdir().unwrap();
        let item = make_item("no attachments", 12, false);
        save_item(dir.path(), "empty_attach.json", &item)
            .await
            .unwrap();

        let json = tokio::fs::read_to_string(dir.path().join("empty_attach.json"))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            v.get("attachments").is_none(),
            "empty attachments should be omitted from JSON"
        );
    }

    #[tokio::test]
    async fn attachments_roundtrip_populated() {
        let dir = tempfile::tempdir().unwrap();
        let mut item = make_item("with attachments", 12, false);
        item.attachments = vec![
            PathBuf::from("inbox/photo.jpg"),
            PathBuf::from("inbox/doc.pdf"),
        ];

        save_item(dir.path(), "with_attach.json", &item)
            .await
            .unwrap();
        let loaded = load_item(&dir.path().join("with_attach.json"))
            .await
            .unwrap();

        assert_eq!(loaded.attachments.len(), 2);
        assert_eq!(loaded.attachments[0], PathBuf::from("inbox/photo.jpg"));
        assert_eq!(loaded.attachments[1], PathBuf::from("inbox/doc.pdf"));
    }

    #[test]
    fn sanitize_attachment_filename_strips_separators() {
        assert_eq!(sanitize_attachment_filename("report.pdf"), "report.pdf");
        assert_eq!(
            sanitize_attachment_filename("a/b\\c.txt"),
            "abc.txt",
            "path separators should be stripped, not preserved as path structure"
        );
    }

    #[test]
    fn sanitize_attachment_filename_rejects_dot_names() {
        assert_eq!(sanitize_attachment_filename(".."), "attachment");
        assert_eq!(sanitize_attachment_filename("."), "attachment");
        assert_eq!(sanitize_attachment_filename(""), "attachment");
        assert_eq!(sanitize_attachment_filename("   "), "attachment");
    }

    #[test]
    fn dedupe_filename_no_collision() {
        let used = HashSet::new();
        assert_eq!(dedupe_filename("report.pdf", &used), "report.pdf");
    }

    #[test]
    fn dedupe_filename_appends_suffix_on_collision() {
        let mut used = HashSet::new();
        used.insert("report.pdf".to_string());
        assert_eq!(dedupe_filename("report.pdf", &used), "report_2.pdf");

        used.insert("report_2.pdf".to_string());
        assert_eq!(dedupe_filename("report.pdf", &used), "report_3.pdf");
    }

    #[test]
    fn dedupe_filename_no_extension() {
        let mut used = HashSet::new();
        used.insert("README".to_string());
        assert_eq!(dedupe_filename("README", &used), "README_2");
    }

    #[tokio::test]
    async fn copy_attachments_empty_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let attachments_dir = dir.path().join("attachments");
        let (recorded, failures) = copy_attachments(&attachments_dir, "item1", &[])
            .await
            .unwrap();
        assert!(recorded.is_empty());
        assert!(failures.is_empty());
        assert!(
            !attachments_dir.exists(),
            "no directory should be created when there are no attachments"
        );
    }

    #[tokio::test]
    async fn copy_attachments_copies_and_records_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("sources");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_file = source_dir.join("report.pdf");
        tokio::fs::write(&source_file, b"pdf bytes").await.unwrap();

        let attachments_dir = dir.path().join("attachments");
        let (recorded, failures) = copy_attachments(
            &attachments_dir,
            "item1",
            std::slice::from_ref(&source_file),
        )
        .await
        .unwrap();

        assert!(failures.is_empty());
        assert_eq!(
            recorded,
            vec![PathBuf::from("inbox/user/attachments/item1/report.pdf")]
        );

        let copied = attachments_dir.join("item1").join("report.pdf");
        assert!(copied.exists(), "attachment should be copied");
        assert!(source_file.exists(), "source file should be left in place");
        let copied_bytes = tokio::fs::read(&copied).await.unwrap();
        assert_eq!(copied_bytes, b"pdf bytes");
    }

    #[tokio::test]
    async fn copy_attachments_survives_source_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let source_file = dir.path().join("ephemeral.txt");
        tokio::fs::write(&source_file, b"gone soon").await.unwrap();

        let attachments_dir = dir.path().join("attachments");
        copy_attachments(
            &attachments_dir,
            "item1",
            std::slice::from_ref(&source_file),
        )
        .await
        .unwrap();

        tokio::fs::remove_file(&source_file).await.unwrap();

        let copied = attachments_dir.join("item1").join("ephemeral.txt");
        assert!(
            copied.exists(),
            "copied attachment should survive source deletion"
        );
    }

    #[tokio::test]
    async fn copy_attachments_dedupes_same_filename() {
        let dir = tempfile::tempdir().unwrap();
        let dir_a = dir.path().join("a");
        let dir_b = dir.path().join("b");
        tokio::fs::create_dir_all(&dir_a).await.unwrap();
        tokio::fs::create_dir_all(&dir_b).await.unwrap();
        let file_a = dir_a.join("report.pdf");
        let file_b = dir_b.join("report.pdf");
        tokio::fs::write(&file_a, b"first").await.unwrap();
        tokio::fs::write(&file_b, b"second").await.unwrap();

        let attachments_dir = dir.path().join("attachments");
        let (recorded, failures) = copy_attachments(&attachments_dir, "item1", &[file_a, file_b])
            .await
            .unwrap();

        assert!(failures.is_empty());
        assert_eq!(recorded.len(), 2);
        let item_dir = attachments_dir.join("item1");
        assert!(item_dir.join("report.pdf").exists());
        assert!(
            item_dir.join("report_2.pdf").exists(),
            "second file with the same name should not clobber the first"
        );
        assert_eq!(
            tokio::fs::read(item_dir.join("report.pdf")).await.unwrap(),
            b"first"
        );
        assert_eq!(
            tokio::fs::read(item_dir.join("report_2.pdf"))
                .await
                .unwrap(),
            b"second"
        );
    }

    #[tokio::test]
    async fn copy_attachments_sanitizes_traversal_style_name() {
        // `FileAttachment::from_path` already derives the filename via
        // `Path::file_name`, which strips directory components — this test
        // guards that behavior end to end through `copy_attachments`.
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        let source_file = nested.join("passwd");
        tokio::fs::write(&source_file, b"not actually /etc/passwd")
            .await
            .unwrap();

        let attachments_dir = dir.path().join("attachments");
        let (recorded, failures) = copy_attachments(&attachments_dir, "item1", &[source_file])
            .await
            .unwrap();

        assert!(failures.is_empty());
        assert_eq!(
            recorded,
            vec![PathBuf::from("inbox/user/attachments/item1/passwd")]
        );
        let item_dir = attachments_dir.join("item1");
        let entries: Vec<_> = std::fs::read_dir(&item_dir).unwrap().collect();
        assert_eq!(entries.len(), 1, "only the sanitized file should exist");
    }

    #[tokio::test]
    async fn copy_attachments_accepts_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let source_file = dir.path().join("huge.bin");
        // Sparse file: reports a large length via metadata without writing
        // real bytes, so the test stays fast. There is no size cap on a
        // local file being attached.
        let file = tokio::fs::File::create(&source_file).await.unwrap();
        file.set_len(30 * 1024 * 1024).await.unwrap();
        drop(file);

        let attachments_dir = dir.path().join("attachments");
        let (recorded, failures) = copy_attachments(&attachments_dir, "item1", &[source_file])
            .await
            .unwrap();

        assert!(failures.is_empty());
        assert_eq!(recorded.len(), 1);
    }

    #[tokio::test]
    async fn copy_attachments_missing_source_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.txt");
        tokio::fs::write(&good, b"here").await.unwrap();
        let missing = dir.path().join("does_not_exist.txt");

        let attachments_dir = dir.path().join("attachments");
        let (recorded, failures) = copy_attachments(&attachments_dir, "item1", &[good, missing])
            .await
            .unwrap();

        assert_eq!(
            recorded,
            vec![PathBuf::from("inbox/user/attachments/item1/good.txt")],
            "the good file should still be copied despite the other failing"
        );
        assert_eq!(failures.len(), 1, "the missing source should be reported");
        assert!(
            attachments_dir.join("item1").join("good.txt").exists(),
            "the successfully-copied file must not be removed because another file failed"
        );
    }

    #[tokio::test]
    async fn quick_add_with_attachments_empty_matches_quick_add() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        let attachments_dir = dir.path().join("attachments");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let (filename, failures) = quick_add_with_attachments(
            &inbox_dir,
            &attachments_dir,
            "no attachments here",
            "body text",
            "agent",
            chrono_tz::UTC,
            &[],
        )
        .await
        .unwrap();

        assert!(failures.is_empty());
        let item = load_item(&inbox_dir.join(&filename)).await.unwrap();
        assert_eq!(item.title, "no attachments here");
        assert!(item.attachments.is_empty());
        assert!(
            !attachments_dir.exists(),
            "no attachments directory should be created"
        );
    }

    #[tokio::test]
    async fn quick_add_with_attachments_records_and_saves() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        let attachments_dir = dir.path().join("attachments");
        let source_file = dir.path().join("photo.jpg");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        tokio::fs::write(&source_file, b"jpeg bytes").await.unwrap();

        let (filename, failures) = quick_add_with_attachments(
            &inbox_dir,
            &attachments_dir,
            "with a photo",
            "body text",
            "agent",
            chrono_tz::UTC,
            &[source_file],
        )
        .await
        .unwrap();

        assert!(failures.is_empty());
        let item = load_item(&inbox_dir.join(&filename)).await.unwrap();
        let item_id = filename.trim_end_matches(".json");
        assert_eq!(item.attachments.len(), 1);
        assert_eq!(
            item.attachments[0],
            PathBuf::from(format!("inbox/user/attachments/{item_id}/photo.jpg"))
        );
    }

    #[tokio::test]
    async fn archive_item_moves_attachments_dir() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("inbox/user");
        let archive = dir.path().join("archive/inbox/user");
        tokio::fs::create_dir_all(&inbox).await.unwrap();

        let mut item = make_item("with attachment", 12, false);
        item.attachments = vec![PathBuf::from("inbox/user/attachments/to_archive/note.txt")];
        save_item(&inbox, "to_archive.json", &item).await.unwrap();

        let attachments_dir = inbox.join("attachments").join("to_archive");
        tokio::fs::create_dir_all(&attachments_dir).await.unwrap();
        tokio::fs::write(attachments_dir.join("note.txt"), b"hello")
            .await
            .unwrap();

        archive_item(&inbox, &archive, "to_archive").await.unwrap();

        assert!(
            !attachments_dir.exists(),
            "source attachments dir should be gone"
        );
        let archived_attachment = archive
            .join("attachments")
            .join("to_archive")
            .join("note.txt");
        assert!(
            archived_attachment.exists(),
            "attachment should be moved into the archive"
        );
        assert_eq!(
            tokio::fs::read(&archived_attachment).await.unwrap(),
            b"hello"
        );
    }

    #[tokio::test]
    async fn archive_item_without_attachments_dir_is_unaffected() {
        // Regression guard: archiving an item with no attachments/<id>/ directory
        // (e.g. every agent-inbox item) must not error just because that directory
        // doesn't exist.
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("inbox/agent");
        let archive = dir.path().join("archive/inbox/agent");
        tokio::fs::create_dir_all(&inbox).await.unwrap();

        save_item(&inbox, "plain.json", &make_item("plain", 12, false))
            .await
            .unwrap();

        archive_item(&inbox, &archive, "plain").await.unwrap();

        assert!(archive.join("plain.json").exists());
        assert!(!archive.join("attachments").exists());
    }
}
