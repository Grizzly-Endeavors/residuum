//! Daily log: timestamped note-taking to `memory/YYYY-MM-DD.md` files.

use std::path::{Path, PathBuf};

use chrono::Local;

use crate::error::IronclawError;

/// Get the path for today's daily log file.
#[must_use]
pub fn daily_log_path(memory_dir: &Path) -> PathBuf {
    let date = Local::now().format("%Y-%m-%d");
    memory_dir.join(format!("{date}.md"))
}

/// Append a timestamped note to today's daily log file.
///
/// Creates the file if it doesn't exist. Each note is prefixed with
/// a timestamp in `HH:MM` format.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub async fn append_daily_note(memory_dir: &Path, note: &str) -> Result<String, IronclawError> {
    let path = daily_log_path(memory_dir);
    let timestamp = Local::now().format("%H:%M");

    let entry = format!("- **{timestamp}** {note}\n");

    // Read existing content or start fresh
    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let header = format!("# {}\n\n", Local::now().format("%Y-%m-%d"));
            header
        }
        Err(e) => {
            return Err(IronclawError::Memory(format!(
                "failed to read daily log at {}: {e}",
                path.display()
            )));
        }
    };

    let content = format!("{existing}{entry}");

    tokio::fs::write(&path, &content).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to write daily log at {}: {e}",
            path.display()
        ))
    })?;

    Ok(format!("note added to {}", path.display()))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn daily_log_path_format() {
        let dir = Path::new("/tmp/memory");
        let path = daily_log_path(dir);
        let path_str = path.to_string_lossy();

        assert!(
            path_str.starts_with("/tmp/memory/"),
            "should be in memory dir"
        );
        assert!(path_str.ends_with(".md"), "should be a markdown file");
        // Verify date format YYYY-MM-DD
        let filename = path.file_stem().unwrap().to_string_lossy();
        assert_eq!(filename.len(), 10, "date should be YYYY-MM-DD format");
    }

    #[tokio::test]
    async fn append_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = append_daily_note(dir.path(), "test note").await.unwrap();

        assert!(result.contains("note added"), "should confirm addition");

        let path = daily_log_path(dir.path());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("test note"), "should contain the note");
        assert!(content.starts_with("# "), "should have date header");
    }

    #[tokio::test]
    async fn append_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        append_daily_note(dir.path(), "first note").await.unwrap();
        append_daily_note(dir.path(), "second note").await.unwrap();

        let path = daily_log_path(dir.path());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("first note"), "should keep first note");
        assert!(content.contains("second note"), "should append second note");
    }

    #[tokio::test]
    async fn append_has_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        append_daily_note(dir.path(), "timed note").await.unwrap();

        let path = daily_log_path(dir.path());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        // Timestamp is in **HH:MM** format
        assert!(content.contains("**"), "should have bold timestamp markers");
    }
}
