//! Daily log: timestamped note-taking to `memory/daily_log/YYYY-MM/daily-log-DD.md` files.

use std::path::{Path, PathBuf};

use chrono::Local;

use crate::error::IronclawError;

/// Get the path for today's daily log file.
///
/// Returns `{memory_dir}/daily_log/{YYYY-MM}/daily-log-{DD}.md`.
#[must_use]
pub fn daily_log_path(memory_dir: &Path) -> PathBuf {
    let now = Local::now();
    let month_dir = now.format("%Y-%m").to_string();
    let day = now.format("%d").to_string();
    memory_dir
        .join("daily_log")
        .join(month_dir)
        .join(format!("daily-log-{day}.md"))
}

/// Append a timestamped note to today's daily log file.
///
/// Creates the file and any missing parent directories if they don't exist.
/// Each note is prefixed with a timestamp in `HH:MM` format.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub async fn append_daily_note(memory_dir: &Path, note: &str) -> Result<String, IronclawError> {
    let path = daily_log_path(memory_dir);
    let timestamp = Local::now().format("%H:%M");

    let entry = format!("- **{timestamp}** {note}\n");

    // Ensure the month subdirectory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            IronclawError::Memory(format!(
                "failed to create daily log directory at {}: {e}",
                parent.display()
            ))
        })?;
    }

    // Read existing content or start fresh
    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(file_content) => file_content,
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

    let file_content = format!("{existing}{entry}");

    tokio::fs::write(&path, &file_content).await.map_err(|e| {
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
            path_str.contains("/daily_log/"),
            "should be inside daily_log subdirectory"
        );
        assert!(path_str.ends_with(".md"), "should be a markdown file");

        let filename = path.file_name().unwrap().to_string_lossy();
        assert!(
            filename.starts_with("daily-log-"),
            "filename should start with daily-log-"
        );
    }

    #[tokio::test]
    async fn append_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = append_daily_note(dir.path(), "test note").await.unwrap();

        assert!(result.contains("note added"), "should confirm addition");

        let path = daily_log_path(dir.path());
        let file_content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(
            file_content.contains("test note"),
            "should contain the note"
        );
        assert!(file_content.starts_with("# "), "should have date header");
    }

    #[tokio::test]
    async fn append_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // The daily_log/YYYY-MM dir doesn't exist yet — append should create it
        let result = append_daily_note(dir.path(), "parent dir test")
            .await
            .unwrap();
        assert!(result.contains("note added"), "should succeed");

        let path = daily_log_path(dir.path());
        assert!(path.exists(), "file should exist after append");
        assert!(
            path.parent().unwrap().exists(),
            "month directory should be created"
        );
    }

    #[tokio::test]
    async fn append_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        append_daily_note(dir.path(), "first note").await.unwrap();
        append_daily_note(dir.path(), "second note").await.unwrap();

        let path = daily_log_path(dir.path());
        let file_content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(
            file_content.contains("first note"),
            "should keep first note"
        );
        assert!(
            file_content.contains("second note"),
            "should append second note"
        );
    }

    #[tokio::test]
    async fn append_has_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        append_daily_note(dir.path(), "timed note").await.unwrap();

        let path = daily_log_path(dir.path());
        let file_content = tokio::fs::read_to_string(&path).await.unwrap();
        // Timestamp is in **HH:MM** format
        assert!(
            file_content.contains("**"),
            "should have bold timestamp markers"
        );
    }
}
