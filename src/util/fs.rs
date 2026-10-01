//! Filesystem utilities.

use std::io;
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use tokio::time::Instant;

/// Write `data` to `path` atomically (temp file in the same directory, then rename).
///
/// The temporary file is named `.{filename}.{random}.residuum-tmp` in the same
/// directory as `path`, so concurrent writers to one path never share a temp
/// file, and [`is_atomic_write_temp`] can recognize it (file watchers skip it).
///
/// # Errors
/// Returns an error if the parent directory is missing, or if writing or renaming fails.
pub(crate) async fn atomic_write(path: &Path, data: impl AsRef<[u8]>) -> anyhow::Result<()> {
    write_via_temp_file(path, data.as_ref(), false).await
}

/// [`atomic_write`] for a file only the current user may read: the temporary
/// file is created with mode 0600 (an owner-only ACL on Windows) before any
/// data goes into it, so the content is never readable by other users, not
/// even briefly. The data is flushed to disk before the rename.
///
/// # Errors
/// Returns an error if the parent directory is missing, or if writing,
/// restricting or renaming fails.
pub(crate) async fn atomic_write_owner_only(
    path: &Path,
    data: impl AsRef<[u8]>,
) -> anyhow::Result<()> {
    write_via_temp_file(path, data.as_ref(), true).await
}

async fn write_via_temp_file(path: &Path, data: &[u8], owner_only: bool) -> anyhow::Result<()> {
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

    let written = if owner_only {
        write_owner_only(&tmp_path, data).await
    } else {
        tokio::fs::write(&tmp_path, data).await
    };
    written.with_context(|| format!("failed to write temporary file at {}", tmp_path.display()))?;

    if owner_only && let Err(e) = crate::config::secrets::set_file_mode_600(&tmp_path) {
        remove_temp_file(&tmp_path).await;
        return Err(anyhow::anyhow!("{e}"));
    }

    if let Err(e) = tokio::fs::rename(&tmp_path, path).await {
        remove_temp_file(&tmp_path).await;
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

/// Delete a temporary file a failed write leaves behind, logging when even
/// that fails.
async fn remove_temp_file(tmp_path: &Path) {
    if let Err(cleanup) = tokio::fs::remove_file(tmp_path).await {
        tracing::warn!(path = %tmp_path.display(), error = %cleanup, "failed to remove temporary file after a failed write");
    }
}

/// Create `path`, which must not exist, readable only by the current user
/// where the platform says so at creation (mode 0600 on Unix), and fill it
/// with `data`.
async fn write_owner_only(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(data).await?;
    file.sync_all().await
}

const ATOMIC_WRITE_TEMP_SUFFIX: &str = ".residuum-tmp";

/// Whether `file_name` is a temporary file [`atomic_write`] creates and
/// renames away.
#[must_use]
pub(crate) fn is_atomic_write_temp(file_name: &str) -> bool {
    file_name.starts_with('.') && file_name.ends_with(ATOMIC_WRITE_TEMP_SUFFIX)
}

/// How long [`rename_dir_when_released`] waits for programs to let go of the
/// files under a directory before it gives up.
const RENAME_RELEASE_WINDOW: Duration = Duration::from_secs(5);

/// How long [`rename_dir_when_released`] waits between attempts.
const RENAME_RELEASE_INTERVAL: Duration = Duration::from_millis(50);

/// The Windows errors for a file that another program has open:
/// `ERROR_ACCESS_DENIED` and `ERROR_SHARING_VIOLATION`.
const WINDOWS_FILE_IN_USE: [i32; 2] = [5, 32];

/// Whether `error` is Windows refusing to move a directory because a program
/// has a file under it open.
pub(crate) fn is_held_open(error: &io::Error) -> bool {
    cfg!(windows)
        && error
            .raw_os_error()
            .is_some_and(|code| WINDOWS_FILE_IN_USE.contains(&code))
}

/// Rename the directory `from` to `to`, waiting for any program that has a
/// file under `from` open to let go of it.
///
/// Windows refuses to rename a directory while a program holds a file under
/// it open, and the programs that do are brief about it: the hub's own
/// readers, a virus scanner looking at a file just written, an indexer, the
/// last worker of an agent that has stopped. Catching one at that moment
/// fails a rename that succeeds a few milliseconds later, so the rename is
/// tried again every 50 milliseconds for up to five seconds. The wait is
/// logged when it starts and when it ends. Nowhere else does a refusal wait,
/// and a refusal that outlasts the wait is returned as it came.
///
/// # Errors
/// Returns the error of the last rename attempt.
pub(crate) async fn rename_dir_when_released(from: &Path, to: &Path) -> io::Result<()> {
    rename_dir_retrying(
        from,
        to,
        is_held_open,
        RENAME_RELEASE_WINDOW,
        RENAME_RELEASE_INTERVAL,
    )
    .await
}

/// [`rename_dir_when_released`] for the refusals `is_held` names, over `window`
/// and `interval`.
async fn rename_dir_retrying(
    from: &Path,
    to: &Path,
    is_held: fn(&io::Error) -> bool,
    window: Duration,
    interval: Duration,
) -> io::Result<()> {
    let started = Instant::now();
    let mut attempts = 0_u32;
    loop {
        attempts += 1;
        match tokio::fs::rename(from, to).await {
            Ok(()) => {
                if attempts > 1 {
                    tracing::info!(
                        from = %from.display(),
                        attempts,
                        waited_ms = started.elapsed().as_millis(),
                        "the directory was released and could be renamed"
                    );
                }
                return Ok(());
            }
            Err(error) if is_held(&error) && started.elapsed() < window => {
                if attempts == 1 {
                    tracing::warn!(
                        from = %from.display(),
                        error = %error,
                        window_ms = window.as_millis(),
                        "a program has a file under the directory open; waiting for it to let go"
                    );
                }
                tokio::time::sleep(interval).await;
            }
            Err(error) => {
                if attempts > 1 {
                    tracing::warn!(
                        from = %from.display(),
                        error = %error,
                        attempts,
                        waited_ms = started.elapsed().as_millis(),
                        "the directory could not be renamed after waiting"
                    );
                }
                return Err(error);
            }
        }
    }
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
            crate::util::spawn_in_span(async move { atomic_write(&target, format!("{i}")).await })
        });
        for write in writes {
            write.await.unwrap().unwrap();
        }

        let content = tokio::fs::read_to_string(&target).await.unwrap();
        assert!(content.parse::<u32>().unwrap() < 16);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owner_only_write_creates_a_private_file_and_replaces_it_whole() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("secret.key");

        atomic_write_owner_only(&target, b"first").await.unwrap();
        atomic_write_owner_only(&target, b"second").await.unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["secret.key"], "temp file should be renamed away");
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

    /// Rename onto a directory that has something in it until `release_after`,
    /// which a directory can't be renamed onto.
    async fn rename_onto_occupied_dir(
        is_held: fn(&io::Error) -> bool,
        release_after: Duration,
    ) -> (io::Result<()>, Duration, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let from = root.path().join("from");
        let to = root.path().join("to");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("kept.txt"), "kept").unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join("occupant.txt"), "occupant").unwrap();

        let releaser = {
            let to = to.clone();
            crate::util::spawn_in_span(async move {
                tokio::time::sleep(release_after).await;
                tokio::fs::remove_dir_all(&to).await.unwrap();
            })
        };
        let started = Instant::now();
        let renamed = rename_dir_retrying(
            &from,
            &to,
            is_held,
            Duration::from_secs(5),
            Duration::from_millis(50),
        )
        .await;
        let waited = started.elapsed();
        releaser.await.unwrap();
        (renamed, waited, root)
    }

    fn occupied(error: &io::Error) -> bool {
        error.kind() == io::ErrorKind::DirectoryNotEmpty
            || error.kind() == io::ErrorKind::AlreadyExists
    }

    fn never_held(_error: &io::Error) -> bool {
        false
    }

    #[tokio::test(start_paused = true)]
    async fn a_directory_that_is_held_for_a_while_is_renamed_once_it_is_released() {
        let (renamed, waited, root) =
            rename_onto_occupied_dir(occupied, Duration::from_millis(400)).await;

        renamed.unwrap();
        assert!(
            waited >= Duration::from_millis(400) && waited < Duration::from_secs(1),
            "waited {waited:?} for a hold that lasted 400ms"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("to").join("kept.txt")).unwrap(),
            "kept"
        );
        assert!(!root.path().join("from").exists());
    }

    #[tokio::test(start_paused = true)]
    async fn a_directory_that_stays_held_fails_with_its_own_error_after_the_window() {
        let (renamed, waited, root) =
            rename_onto_occupied_dir(occupied, Duration::from_secs(60)).await;

        let error = renamed.unwrap_err();
        assert!(occupied(&error), "{error:?}");
        assert!(
            waited >= Duration::from_secs(5) && waited < Duration::from_secs(6),
            "waited {waited:?} for a five second window"
        );
        assert!(
            root.path().join("from").join("kept.txt").exists(),
            "a rename that never happened leaves the directory where it was"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_that_is_not_a_hold_fails_at_once() {
        let (renamed, waited, _root) =
            rename_onto_occupied_dir(never_held, Duration::from_millis(400)).await;

        assert!(occupied(&renamed.unwrap_err()));
        assert_eq!(waited, Duration::ZERO, "nothing to wait for");
    }

    #[test]
    fn only_windows_file_in_use_errors_are_a_hold() {
        let access_denied = io::Error::from_raw_os_error(5);
        let sharing_violation = io::Error::from_raw_os_error(32);
        let missing = io::Error::from(io::ErrorKind::NotFound);

        assert_eq!(is_held_open(&access_denied), cfg!(windows));
        assert_eq!(is_held_open(&sharing_violation), cfg!(windows));
        assert!(!is_held_open(&missing));
    }
}
