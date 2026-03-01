//! Inbox system: a general-purpose "deal with it later" queue.
//!
//! Items are stored as individual JSON files in the workspace `inbox/` directory.
//! External systems can add items by dropping `.json` files into the directory.

use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

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
    /// Paths to related files, relative to the workspace root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PathBuf>,
}

/// Generate a filename for an inbox item: `{YYYYMMDD}_{sanitized_title}.json`.
///
/// Sanitizes: lowercase, non-alphanumeric → `_`, collapse consecutive `_`, truncate to 60 chars.
#[must_use]
pub fn generate_filename(title: &str, tz: chrono_tz::Tz) -> String {
    let now = crate::time::now_local(tz);
    let date = now.format("%Y%m%d").to_string();

    let sanitized: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();

    // Collapse consecutive underscores
    let mut collapsed = String::with_capacity(sanitized.len());
    let mut prev_underscore = false;
    for ch in sanitized.chars() {
        if ch == '_' {
            if !prev_underscore {
                collapsed.push('_');
            }
            prev_underscore = true;
        } else {
            collapsed.push(ch);
            prev_underscore = false;
        }
    }

    // Trim leading/trailing underscores and truncate
    let trimmed = collapsed.trim_matches('_');
    let truncated: String = trimmed.chars().take(60).collect();
    let truncated = truncated.trim_end_matches('_');

    format!("{date}_{truncated}.json")
}

/// Add an inbox item in one call: generates a filename, builds the item, and saves.
///
/// Returns the filename for confirmation messages.
///
/// # Errors
/// Returns an error if the item cannot be saved.
pub async fn quick_add(
    inbox_dir: &Path,
    title: &str,
    body: &str,
    source: &str,
    tz: chrono_tz::Tz,
) -> anyhow::Result<String> {
    let now = crate::time::now_local(tz);
    let filename = generate_filename(title, tz);
    let item = InboxItem {
        title: title.to_string(),
        body: body.to_string(),
        source: source.to_string(),
        timestamp: now,
        read: false,
        attachments: Vec::new(),
    };
    save_item(inbox_dir, &filename, &item).await?;
    Ok(filename)
}

/// Save an inbox item atomically (write to `.tmp`, then rename).
///
/// # Errors
/// Returns an error if serialization or file operations fail.
pub async fn save_item(inbox_dir: &Path, filename: &str, item: &InboxItem) -> anyhow::Result<()> {
    let target = inbox_dir.join(filename);
    let tmp = inbox_dir.join(format!(".{filename}.tmp"));

    let json = serde_json::to_string_pretty(item)?;
    tokio::fs::write(&tmp, json).await?;
    tokio::fs::rename(&tmp, &target).await?;

    Ok(())
}

/// Load a single inbox item from a JSON file.
///
/// # Errors
/// Returns an error if the file cannot be read or parsed.
pub async fn load_item(path: &Path) -> anyhow::Result<InboxItem> {
    let content = tokio::fs::read_to_string(path).await?;
    let item: InboxItem = serde_json::from_str(&content)?;
    Ok(item)
}

/// List all inbox items (non-recursive, ignores subdirectories).
///
/// Returns `(filename_stem, item)` pairs sorted newest-first by timestamp.
///
/// # Errors
/// Returns an error if the directory cannot be read.
pub async fn list_items(inbox_dir: &Path) -> anyhow::Result<Vec<(String, InboxItem)>> {
    let mut entries = Vec::new();
    let mut dir = tokio::fs::read_dir(inbox_dir).await?;

    while let Some(entry) = dir.next_entry().await? {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
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

    entries.sort_by(|a, b| b.1.timestamp.cmp(&a.1.timestamp));
    Ok(entries)
}

/// Count unread inbox items.
///
/// Deserializes each `.json` file in the inbox directory — acceptable for low throughput.
#[must_use]
pub fn count_unread(inbox_dir: &Path) -> usize {
    let Ok(dir) = std::fs::read_dir(inbox_dir) else {
        return 0;
    };

    dir.filter_map(Result::ok)
        .filter(|e| {
            let p = e.path();
            p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("json")
        })
        .filter(|e| {
            std::fs::read_to_string(e.path())
                .ok()
                .and_then(|c| serde_json::from_str::<InboxItem>(&c).ok())
                .is_some_and(|item| !item.read)
        })
        .count()
}

/// Mark an inbox item as read and save it back atomically.
///
/// # Errors
/// Returns an error if the file cannot be found, read, or written.
pub async fn mark_read(inbox_dir: &Path, filename: &str) -> anyhow::Result<InboxItem> {
    let json_name = ensure_json_ext(filename);
    let path = inbox_dir.join(&json_name);

    let mut item = load_item(&path).await?;
    item.read = true;
    save_item(inbox_dir, &json_name, &item).await?;

    Ok(item)
}

/// Move an inbox item to the inbox archive directory (`archive/inbox/`).
///
/// # Errors
/// Returns an error if the file is not found or the move fails.
pub async fn archive_item(
    inbox_dir: &Path,
    archive_dir: &Path,
    filename: &str,
) -> anyhow::Result<()> {
    let json_name = ensure_json_ext(filename);
    let src = inbox_dir.join(&json_name);

    if !src.exists() {
        anyhow::bail!("inbox item '{json_name}' not found");
    }

    tokio::fs::create_dir_all(archive_dir).await?;
    let dst = archive_dir.join(&json_name);
    tokio::fs::rename(&src, &dst).await?;

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
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::string_slice,
    reason = "test code uses unwrap, indexing, and string slicing for clarity"
)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn make_item(title: &str, read: bool) -> InboxItem {
        InboxItem {
            title: title.to_string(),
            body: format!("Body for {title}"),
            source: "test".to_string(),
            timestamp: NaiveDate::from_ymd_opt(2026, 2, 25)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
            read,
            attachments: Vec::new(),
        }
    }

    fn make_item_at(title: &str, hour: u32) -> InboxItem {
        InboxItem {
            title: title.to_string(),
            body: format!("Body for {title}"),
            source: "test".to_string(),
            timestamp: NaiveDate::from_ymd_opt(2026, 2, 25)
                .unwrap()
                .and_hms_opt(hour, 0, 0)
                .unwrap(),
            read: false,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn generate_filename_basic() {
        let name = generate_filename("Hello World", chrono_tz::UTC);
        // Date prefix + sanitized title
        assert!(
            name.ends_with("_hello_world.json"),
            "should sanitize title: {name}"
        );
        assert!(
            name.len() > "20260225_".len(),
            "should have date prefix: {name}"
        );
    }

    #[test]
    fn generate_filename_special_chars() {
        let name = generate_filename("foo/bar\\baz..qux!!", chrono_tz::UTC);
        assert!(!name.contains('/'), "should not contain slashes: {name}");
        assert!(
            !name.contains('\\'),
            "should not contain backslashes: {name}"
        );
        assert!(!name.contains("__"), "should collapse underscores: {name}");
    }

    #[test]
    fn generate_filename_unicode() {
        let name = generate_filename("café résumé", chrono_tz::UTC);
        assert!(
            Path::new(&name)
                .extension()
                .is_some_and(|ext| ext == "json"),
            "should end with .json: {name}"
        );
        assert!(!name.contains("__"), "should collapse underscores: {name}");
    }

    #[test]
    fn generate_filename_truncation() {
        let long_title = "a".repeat(100);
        let name = generate_filename(&long_title, chrono_tz::UTC);
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
        let item = make_item("test roundtrip", false);

        save_item(dir.path(), "test.json", &item).await.unwrap();
        let loaded = load_item(&dir.path().join("test.json")).await.unwrap();

        assert_eq!(loaded.title, "test roundtrip");
        assert_eq!(loaded.body, "Body for test roundtrip");
        assert_eq!(loaded.source, "test");
        assert!(!loaded.read);
    }

    #[tokio::test]
    async fn list_items_sorted_by_timestamp() {
        let dir = tempfile::tempdir().unwrap();

        let early = make_item_at("early", 8);
        let late = make_item_at("late", 20);
        let mid = make_item_at("mid", 14);

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
        let item = make_item("valid", false);
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
    async fn list_items_ignores_archive_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let item = make_item("active", false);
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
    async fn count_unread_accuracy() {
        let dir = tempfile::tempdir().unwrap();

        save_item(dir.path(), "unread1.json", &make_item("a", false))
            .await
            .unwrap();
        save_item(dir.path(), "unread2.json", &make_item("b", false))
            .await
            .unwrap();
        save_item(dir.path(), "read1.json", &make_item("c", true))
            .await
            .unwrap();

        assert_eq!(
            count_unread(dir.path()),
            2,
            "should count only unread items"
        );
    }

    #[tokio::test]
    async fn count_unread_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(count_unread(dir.path()), 0);
    }

    #[tokio::test]
    async fn count_unread_missing_dir() {
        let missing = Path::new("/tmp/nonexistent_inbox_dir_test");
        assert_eq!(count_unread(missing), 0, "missing dir should return 0");
    }

    #[tokio::test]
    async fn mark_read_updates_file() {
        let dir = tempfile::tempdir().unwrap();
        save_item(dir.path(), "item.json", &make_item("test", false))
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

        save_item(&inbox, "to_archive.json", &make_item("archive me", false))
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
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "error should mention not found: {err}"
        );
    }

    #[tokio::test]
    async fn attachments_roundtrip_empty() {
        let dir = tempfile::tempdir().unwrap();
        let item = make_item("no attachments", false);
        save_item(dir.path(), "empty_attach.json", &item)
            .await
            .unwrap();

        let loaded = load_item(&dir.path().join("empty_attach.json"))
            .await
            .unwrap();
        assert!(
            loaded.attachments.is_empty(),
            "empty attachments should round-trip"
        );
    }

    #[tokio::test]
    async fn attachments_roundtrip_populated() {
        let dir = tempfile::tempdir().unwrap();
        let mut item = make_item("with attachments", false);
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
}
