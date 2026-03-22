//! Filesystem utilities.

use std::path::Path;

use anyhow::Context as _;

/// Write `data` to `path` atomically (temp file in the same directory, then rename).
///
/// The temporary file is named `.{filename}.tmp` in the same directory as `path`.
///
/// # Errors
/// Returns an error if the parent directory is missing, or if writing or renaming fails.
pub(crate) async fn atomic_write(path: &Path, data: impl AsRef<[u8]>) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent directory: {}", path.display()))?;

    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp_path = dir.join(format!(".{filename}.tmp"));

    tokio::fs::write(&tmp_path, data.as_ref())
        .await
        .with_context(|| format!("failed to write temporary file at {}", tmp_path.display()))?;

    tokio::fs::rename(&tmp_path, path).await.with_context(|| {
        format!(
            "failed to rename {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_and_verify_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("data.json");

        atomic_write(&target, b"hello").await.unwrap();

        let content = tokio::fs::read_to_string(&target).await.unwrap();
        assert_eq!(content, "hello");

        // Temp file should not remain
        let tmp = dir.path().join(".data.json.tmp");
        assert!(!tmp.exists(), "temp file should be cleaned up after rename");
    }

    #[tokio::test]
    async fn nonexistent_parent_returns_error() {
        let path = Path::new("/nonexistent/dir/file.json");
        let result = atomic_write(path, b"data").await;
        assert!(result.is_err(), "should fail when parent dir is missing");
    }
}
