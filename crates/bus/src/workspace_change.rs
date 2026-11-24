use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceChange {
    pub path: String,
    pub kind: WorkspaceChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkspaceChangeKind {
    Created,
    Modified,
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
    WatcherRestarted,
}
