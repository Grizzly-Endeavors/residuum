//! Polling-based file watcher for workspace config hot-reload.

use std::path::PathBuf;
use std::time::SystemTime;

use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

use super::ReloadSignal;

/// Tracks a file's modification time for change detection.
struct WatchedFile {
    path: PathBuf,
    last_mtime: Option<SystemTime>,
}

impl WatchedFile {
    fn new(path: PathBuf) -> Self {
        let last_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Self { path, last_mtime }
    }

    /// Check if the file's mtime has changed since the last check.
    ///
    /// Returns `true` if mtime changed (file modified, created, or deleted).
    fn check(&mut self) -> bool {
        let current = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();

        if current == self.last_mtime {
            false
        } else {
            self.last_mtime = current;
            true
        }
    }

    /// Update the stored mtime without returning whether the file changed.
    fn sync_mtime(&mut self) {
        self.check();
    }
}

/// Spawn a polling watcher for workspace config files.
///
/// Polls `mcp_path` and `channels_path` every 2 seconds. When either file's
/// mtime changes, debounces 500ms then sends `ReloadSignal::Workspace`.
pub(super) fn spawn_workspace_watcher(
    mcp_path: PathBuf,
    channels_path: PathBuf,
    reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut mcp_file = WatchedFile::new(mcp_path);
        let mut channels_file = WatchedFile::new(channels_path);
        let mut interval = tokio::time::interval(Duration::from_secs(2));

        // Skip the first immediate tick (files were just loaded at startup)
        interval.tick().await;

        loop {
            interval.tick().await;

            let mcp_changed = mcp_file.check();
            let channels_changed = channels_file.check();

            if mcp_changed || channels_changed {
                tracing::debug!(
                    mcp_changed,
                    channels_changed,
                    "workspace config file change detected, debouncing"
                );

                // Debounce: wait 500ms for any rapid edits to settle
                sleep(Duration::from_millis(500)).await;

                // Re-check to get the settled state
                mcp_file.sync_mtime();
                channels_file.sync_mtime();

                tracing::info!("sending workspace reload signal");
                if reload_tx.send(ReloadSignal::Workspace).is_err() {
                    tracing::debug!("reload receiver dropped, stopping workspace watcher");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn watched_file_detects_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");
        std::fs::write(&path, "initial").unwrap();

        let mut wf = WatchedFile::new(path.clone());

        // First check after construction should return false (no change)
        assert!(!wf.check(), "no change immediately after construction");

        // Force a distinct mtime by setting it to 1 second in the future
        let future = SystemTime::now() + std::time::Duration::from_secs(2);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(future).unwrap();

        assert!(wf.check(), "should detect mtime change after modification");
    }

    #[test]
    fn watched_file_nonexistent() {
        let mut wf = WatchedFile::new(PathBuf::from("/tmp/nonexistent_watcher_test_file"));

        // Starts with None mtime, check returns false (no change from None → None)
        assert!(!wf.check(), "nonexistent file should return false");
    }

    #[test]
    fn watched_file_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new_file.json");

        // File doesn't exist at construction time
        let mut wf = WatchedFile::new(path.clone());
        assert!(
            wf.last_mtime.is_none(),
            "should start with None when file missing"
        );

        // Create the file
        std::fs::write(&path, "created").unwrap();

        // Should detect the creation (None → Some)
        assert!(wf.check(), "should detect file creation");
    }
}
