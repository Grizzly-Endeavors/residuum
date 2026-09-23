//! Filesystem utilities.

use std::path::Path;

use anyhow::Context as _;

/// Write `data` to `path` atomically (temp file in the same directory, then rename).
///
/// The temporary file is named `.{filename}.{random}.residuum-tmp` in the same
/// directory as `path`, so concurrent writers to one path never share a temp
/// file, and [`is_atomic_write_temp`] can recognize it (file watchers skip it).
///
/// # Errors
/// Returns an error if the parent directory is missing, or if writing or renaming fails.
pub(crate) async fn atomic_write(path: &Path, data: impl AsRef<[u8]>) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent directory: {}", path.display()))?;

    let filename = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no filename: {}", path.display()))?
        .to_string_lossy();
    let suffix: u32 = rand::random();
    let tmp_path = dir.join(format!(
        ".{filename}.{suffix:08x}{ATOMIC_WRITE_TEMP_SUFFIX}"
    ));

    tokio::fs::write(&tmp_path, data.as_ref())
        .await
        .with_context(|| format!("failed to write temporary file at {}", tmp_path.display()))?;

    if let Err(e) = tokio::fs::rename(&tmp_path, path).await {
        if let Err(cleanup) = tokio::fs::remove_file(&tmp_path).await {
            tracing::warn!(path = %tmp_path.display(), error = %cleanup, "failed to remove temporary file after a failed rename");
        }
        return Err(e).with_context(|| {
            format!(
                "failed to rename {} to {}",
                tmp_path.display(),
                path.display()
            )
        });
    }

    Ok(())
}

const ATOMIC_WRITE_TEMP_SUFFIX: &str = ".residuum-tmp";

/// Whether `file_name` is a temporary file [`atomic_write`] creates and
/// renames away.
#[must_use]
pub(crate) fn is_atomic_write_temp(file_name: &str) -> bool {
    file_name.starts_with('.') && file_name.ends_with(ATOMIC_WRITE_TEMP_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_and_verify_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("data.json");

        atomic_write(&target, b"hello").await.unwrap();

        let content = tokio::fs::read_to_string(&target).await.unwrap();
        assert_eq!(content, "hello");

        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["data.json"], "temp file should be renamed away");
    }

    #[tokio::test]
    async fn concurrent_writes_to_one_path_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("data.json");

        let writes = (0..16).map(|i| {
            let target = target.clone();
            tokio::spawn(async move { atomic_write(&target, format!("{i}")).await })
        });
        for write in writes {
            write.await.unwrap().unwrap();
        }

        let content = tokio::fs::read_to_string(&target).await.unwrap();
        assert!(content.parse::<u32>().unwrap() < 16);
    }

    #[test]
    fn recognizes_its_own_temp_files() {
        assert!(is_atomic_write_temp(".notes.md.1a2b3c4d.residuum-tmp"));
        assert!(!is_atomic_write_temp("notes.md"));
        assert!(!is_atomic_write_temp(".notes.md.tmp"));
        assert!(!is_atomic_write_temp("notes.residuum-tmp"));
    }

    #[tokio::test]
    async fn nonexistent_parent_returns_error() {
        let path = Path::new("/nonexistent/dir/file.json");
        let result = atomic_write(path, b"data").await;
        assert!(result.is_err(), "should fail when parent dir is missing");
        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("/nonexistent/dir"),
            "error should include the failing path, got: {msg}"
        );
    }

    #[tokio::test]
    async fn path_without_filename_returns_error() {
        let path = Path::new("/");
        let result = atomic_write(path, b"data").await;
        assert!(result.is_err(), "should fail for path with no filename");
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path has no"),
            "error should describe the problem, got: {msg}"
        );
    }

    #[tokio::test]
    async fn overwrite_replaces_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("data.json");

        atomic_write(&target, b"original").await.unwrap();
        atomic_write(&target, b"updated").await.unwrap();

        let content = tokio::fs::read_to_string(&target).await.unwrap();
        assert_eq!(content, "updated");
    }
}
