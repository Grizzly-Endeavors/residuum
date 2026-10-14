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
/// Polls `mcp_path`, `channels_path`, `agent_card_path`, and `a2a_agents_path`
/// every 2 seconds. When any file's mtime changes, debounces 500ms then sends
/// `ReloadSignal::Workspace`.
pub(super) fn spawn_workspace_watcher(
    mcp_path: PathBuf,
    channels_path: PathBuf,
    agent_card_path: PathBuf,
    a2a_agents_path: PathBuf,
    reload_tx: crate::gateway::types::ReloadSender,
) -> JoinHandle<()> {
    // Read before the task starts, so an edit made before it first runs is
    // seen as a change rather than taken as the starting point.
    let mut mcp_file = WatchedFile::new(mcp_path);
    let mut channels_file = WatchedFile::new(channels_path);
    let mut agent_card_file = WatchedFile::new(agent_card_path);
    let mut a2a_agents_file = WatchedFile::new(a2a_agents_path);
    crate::util::spawn_in_span(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));

        // Skip the first immediate tick (files were just loaded at startup)
        interval.tick().await;

        loop {
            interval.tick().await;

            let mcp_changed = mcp_file.check();
            let channels_changed = channels_file.check();
            let agent_card_changed = agent_card_file.check();
            let a2a_agents_changed = a2a_agents_file.check();

            if mcp_changed || channels_changed || agent_card_changed || a2a_agents_changed {
                tracing::debug!(
                    mcp_changed,
                    channels_changed,
                    agent_card_changed,
                    a2a_agents_changed,
                    "workspace config file change detected, debouncing"
                );

                // Debounce: wait 500ms for any rapid edits to settle
                sleep(Duration::from_millis(500)).await;

                // Re-check to get the settled state
                mcp_file.sync_mtime();
                channels_file.sync_mtime();
                agent_card_file.sync_mtime();
                a2a_agents_file.sync_mtime();

                tracing::info!("sending workspace reload signal");
                if reload_tx.send(ReloadSignal::Workspace).is_err() {
                    tracing::debug!("reload receiver dropped, stopping workspace watcher");
                    break;
                }
            }
        }
    })
}

/// Spawn a polling watcher for `config.toml`/`providers.toml`.
///
/// Polls both files every 2 seconds; on either's mtime changing, debounces
/// 500ms then sends `ReloadSignal::Agent` — the same signal the web UI's
/// Settings form and Raw tab already send explicitly after their own
/// writes, so a direct edit (the agent's `write_file`/`edit_file`, or a
/// manual edit outside Residuum entirely) picks up the change the same way.
pub(super) fn spawn_root_config_watcher(
    config_toml_path: PathBuf,
    providers_toml_path: PathBuf,
    reload_tx: crate::gateway::types::ReloadSender,
) -> JoinHandle<()> {
    // Read before the task starts; see `spawn_workspace_watcher`.
    let mut config_file = WatchedFile::new(config_toml_path);
    let mut providers_file = WatchedFile::new(providers_toml_path);
    crate::util::spawn_in_span(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));

        // Skip the first immediate tick (files were just loaded at startup)
        interval.tick().await;

        loop {
            interval.tick().await;

            let config_changed = config_file.check();
            let providers_changed = providers_file.check();

            if config_changed || providers_changed {
                tracing::debug!(
                    config_changed,
                    providers_changed,
                    "root config file change detected, debouncing"
                );

                // Debounce: wait 500ms for any rapid edits to settle
                sleep(Duration::from_millis(500)).await;

                // Re-check to get the settled state
                config_file.sync_mtime();
                providers_file.sync_mtime();

                tracing::info!("sending root config reload signal");
                if reload_tx.send(ReloadSignal::Agent).is_err() {
                    tracing::debug!("reload receiver dropped, stopping root config watcher");
                    break;
                }
            }
        }
    })
}

/// Spawn a polling watcher for `hub/config.toml`.
///
/// Polls every 2 seconds; on the file's mtime changing, debounces 500ms then
/// sends `ReloadSignal::Hub`.
pub(crate) fn spawn_hub_config_watcher(
    hub_config_toml_path: PathBuf,
    reload_tx: crate::gateway::types::ReloadSender,
) -> JoinHandle<()> {
    // Read before the task starts; see `spawn_workspace_watcher`.
    let mut hub_config_file = WatchedFile::new(hub_config_toml_path);
    crate::util::spawn_in_span(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));

        // Skip the first immediate tick (the file was just loaded at startup)
        interval.tick().await;

        loop {
            interval.tick().await;

            if hub_config_file.check() {
                tracing::debug!("hub config file change detected, debouncing");

                sleep(Duration::from_millis(500)).await;
                hub_config_file.sync_mtime();

                tracing::info!("sending hub config reload signal");
                if reload_tx.send(ReloadSignal::Hub).is_err() {
                    tracing::debug!("reload receiver dropped, stopping hub config watcher");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
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

    /// Give `path` a modification time `secs` seconds after the epoch, so
    /// two writes never share one by landing in the same clock tick.
    fn set_mtime(path: &std::path::Path, secs: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_edit_made_before_the_poller_first_runs_is_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let providers = dir.path().join("providers.toml");
        for path in [&config, &providers] {
            std::fs::write(path, "").unwrap();
            set_mtime(path, 1_000);
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let _poller = spawn_root_config_watcher(config.clone(), providers, tx);
        // The poller's task hasn't run yet: this test hasn't yielded.
        set_mtime(&config, 2_000);
        let reload = crate::testing::clock::within(Duration::from_secs(3), rx.recv()).await;

        assert_eq!(reload, Some(Some(ReloadSignal::Agent)));
    }
}
