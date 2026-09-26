//! Workspace change feed: one recursive watcher over the whole workspace.
//!
//! The watcher turns OS file notifications into debounced batches of
//! workspace-relative changes, drops paths the workspace access policy hides,
//! and publishes each batch as a [`crate::bus::WorkspaceEvent`] on
//! [`crate::bus::topics::Workspace`]. WebSocket connections filter batches by
//! their own [`WatchSet`], and the workbench derives artifact reloads from the
//! same stream.
//!
//! - [`batcher`] coalesces raw notifications (300 ms quiet, 2 s at most) and
//!   resolves each touched path to a final change kind.
//! - [`classify`] maps one OS notification to workspace-relative raw changes.
//! - [`feed`] runs the OS watcher (native, falling back to polling) and the
//!   batching loop.
//! - [`watch_set`] owns a connection's prefixes and segment-based matching.

mod batcher;
mod classify;
mod feed;
mod watch_set;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub(crate) use feed::spawn_change_feed;
pub use watch_set::{InvalidWatchPrefix, MAX_CHANGES_PER_FRAME, WatchSet, WatchedChanges};

/// One changed workspace path in a `workspace_changed` frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceChange {
    /// Workspace-relative path, `/`-separated. A directory change stands for
    /// its whole subtree.
    pub path: String,
    /// What happened to the path over the batch.
    pub kind: WorkspaceChangeKind,
}

/// What happened to a path over one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkspaceChangeKind {
    /// The path appeared (including a file renamed or atomically written
    /// into place).
    Created,
    /// The path's content or metadata changed.
    Modified,
    /// The path is gone (including renamed away).
    Removed,
}

/// Why a watching connection's view of the workspace may be stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkspaceResyncReason {
    /// Changes were lost (the OS dropped notifications) or too many matched
    /// one batch to list.
    Overflow,
    /// The watcher was restarted, so changes around the restart may be
    /// missing.
    WatcherRestarted,
}

/// Whether the change feed is running, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// What the web UI shows when live updates are off.
pub const LIVE_UPDATES_OFF_MESSAGE: &str = "Live updates are off: Residuum couldn't watch the workspace for changes, so open artifacts won't refresh on their own. Reload an artifact to see the latest files, and check Residuum's logs for the cause.";
