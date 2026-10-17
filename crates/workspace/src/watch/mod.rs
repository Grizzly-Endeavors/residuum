//! Workspace change feed: one recursive watcher over the whole workspace.
//!
//! The watcher turns OS file notifications into debounced batches of
//! workspace-relative changes, drops paths the workspace access policy hides,
//! and publishes each batch as a [`residuum_bus::WorkspaceEvent`] on
//! [`residuum_bus::topics::Workspace`]. WebSocket connections filter batches by
//! their own [`WatchSet`], and the workbench derives artifact reloads from the
//! same stream.
//!
//! - `batcher` coalesces raw notifications (300 ms quiet, 2 s at most) and
//!   resolves each touched path to a final change kind.
//! - `classify` maps one OS notification to workspace-relative raw changes.
//! - `feed` runs the OS watcher (native, falling back to polling) and the
//!   batching loop.
//! - `watch_set` owns a connection's prefixes and segment-based matching.

mod batcher;
mod classify;
mod feed;
mod watch_set;

use serde::Serialize;

pub use feed::spawn_change_feed;
pub use watch_set::{InvalidWatchPrefix, MAX_CHANGES_PER_FRAME, WatchSet, WatchedChanges};

/// Whether the change feed is running, and how. An agent's
/// `/api/agents/{name}/status` reports it as `live_updates`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchHealth {
    /// The watcher has not started yet.
    Starting,
    /// OS change notifications.
    Native,
    /// The polling fallback, used when native notifications can't start.
    Polling,
    /// No watcher is running: live updates are off.
    Off,
}

/// Assert a started watcher is on native notifications where the platform
/// has them. A watcher that fell back to polling still passes tests, only
/// slower: changes arrive up to two seconds late, which turns into timeouts
/// under load with no hint of the cause. The usual cause on Linux is running
/// out of inotify instances (`fs.inotify.max_user_instances`).
///
/// # Panics
///
/// Panics when the watcher is not on native notifications where the platform has them.
#[cfg(any(test, feature = "test-support"))]
pub fn assert_native_watch(health: WatchHealth, what: &str) {
    assert_ne!(health, WatchHealth::Starting, "{what} hasn't started yet");
    assert_ne!(health, WatchHealth::Off, "{what} couldn't start at all");
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        assert_eq!(
            health,
            WatchHealth::Native,
            "{what} fell back to polling: native file notifications couldn't start (on Linux, check inotify instances in use against fs.inotify.max_user_instances)"
        );
    }
}

/// What the web UI shows when live updates are off.
pub const LIVE_UPDATES_OFF_MESSAGE: &str = "Live updates are off: Residuum couldn't watch the workspace for changes, so open artifacts won't refresh on their own. Reload an artifact to see the latest files, and check Residuum's logs for the cause.";
