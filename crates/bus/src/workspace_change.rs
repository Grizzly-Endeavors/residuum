//! Workspace changes as the bus carries them: batches of changed paths from
//! the workspace change feed, and the signal that a connection's view may be
//! stale.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
