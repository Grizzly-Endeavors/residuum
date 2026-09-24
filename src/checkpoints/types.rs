//! Public data types for the checkpoints system.

use chrono::{DateTime, Utc};

/// Which checkpoint repository an operation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepoKind {
    /// `~/.residuum/checkpoints/workspace.git`, work tree = the workspace root.
    Workspace,
    /// `~/.residuum/checkpoints/config.git`: root `config.toml`, `providers.toml`,
    /// the encrypted secret/agent-key/A2A-key stores. Local-only forever.
    Config,
}

impl RepoKind {
    /// The git-dir file name under the checkpoints directory.
    #[must_use]
    pub(super) fn dir_name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace.git",
            Self::Config => "config.git",
        }
    }
}

/// Why a checkpoint was taken. Recorded as a commit trailer and shown back
/// in every listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckpointTrigger {
    /// Start of an agent turn: captures edits made outside Residuum since
    /// the previous checkpoint.
    TurnStart,
    /// End of an agent turn: captures what the turn itself changed.
    TurnEnd,
    /// Before a destructive workspace API action (delete, overwrite,
    /// move/rename with overwrite, workbench artifact delete).
    PreAction,
    /// Before a write to a root config file or an encrypted key store.
    PreConfigWrite,
    /// Applying a `workspace_restore` path restore.
    Restore,
    /// Undoing a turn's changes.
    Undo,
}

impl CheckpointTrigger {
    /// Stable string used in the commit trailer and the HTTP/tool-facing API.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TurnStart => "turn_start",
            Self::TurnEnd => "turn_end",
            Self::PreAction => "pre_action",
            Self::PreConfigWrite => "pre_config_write",
            Self::Restore => "restore",
            Self::Undo => "undo",
        }
    }

    /// Parse a trailer's stored label back into a trigger. Returns `None`
    /// for anything not written by this version of the checkpoints system
    /// (a forward-compatibility guard, not an expected case today).
    #[must_use]
    pub(super) fn from_label(label: &str) -> Option<Self> {
        match label {
            "turn_start" => Some(Self::TurnStart),
            "turn_end" => Some(Self::TurnEnd),
            "pre_action" => Some(Self::PreAction),
            "pre_config_write" => Some(Self::PreConfigWrite),
            "restore" => Some(Self::Restore),
            "undo" => Some(Self::Undo),
            _ => None,
        }
    }
}

/// Context recorded on every checkpoint commit: which session/address, run
/// id, turn/correlation id, trigger, and a short summary of what happened.
#[derive(Debug, Clone)]
pub struct CheckpointContext {
    /// The session/address that caused this checkpoint (`"main"`, a session
    /// address, or `"system"` for a non-agent-triggered write like a
    /// Settings save).
    pub address: String,
    /// The run id of the turn that caused this checkpoint, when there is
    /// one.
    pub run_id: Option<String>,
    /// The turn or correlation id, when there is one.
    pub turn_id: Option<String>,
    /// Why this checkpoint was taken.
    pub trigger: CheckpointTrigger,
    /// A short, one-line summary (e.g. which tools were called this turn,
    /// or which action triggered the checkpoint).
    pub summary: String,
}

impl CheckpointContext {
    /// Build a context for a non-agent-triggered checkpoint (a Settings
    /// save, a raw config PUT, or similar system-initiated write).
    #[must_use]
    pub fn system(trigger: CheckpointTrigger, summary: impl Into<String>) -> Self {
        Self {
            address: "system".to_string(),
            run_id: None,
            turn_id: None,
            trigger,
            summary: summary.into(),
        }
    }
}

/// One checkpoint as listed or looked up.
#[derive(Debug, Clone)]
pub struct CheckpointSummary {
    /// Full commit hex id.
    pub id: String,
    /// When the checkpoint was taken.
    pub timestamp: DateTime<Utc>,
    /// The session/address that caused it.
    pub address: String,
    /// The run id, when there is one.
    pub run_id: Option<String>,
    /// The turn/correlation id, when there is one.
    pub turn_id: Option<String>,
    /// Why it was taken.
    pub trigger: CheckpointTrigger,
    /// Its short summary.
    pub summary: String,
    /// How many paths changed relative to the checkpoint before it.
    pub changed_path_count: usize,
}

/// How a path changed in a checkpoint, relative to the checkpoint before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Added
    Added,
    /// Modified
    Modified,
    /// Deleted
    Deleted,
}

/// One changed path in a checkpoint's diff.
#[derive(Debug, Clone)]
pub struct ChangedPath {
    /// Path relative to the repository's work tree.
    pub path: String,
    /// How it changed.
    pub kind: ChangeKind,
}

/// A checkpoint plus the paths it changed.
#[derive(Debug, Clone)]
pub struct CheckpointDetail {
    /// The checkpoint itself.
    pub summary: CheckpointSummary,
    /// Paths it changed, relative to the checkpoint before it.
    pub changed_paths: Vec<ChangedPath>,
}

/// On-disk size and history depth of a checkpoint repository, shown in
/// `/api/status` and the CLI so growth is visible before the UI lands.
#[derive(Debug, Clone)]
pub struct RepoStats {
    /// Total size on disk of the repository's git directory, in bytes.
    pub on_disk_bytes: u64,
    /// Total number of checkpoints recorded.
    pub checkpoint_count: u64,
    /// When the oldest checkpoint was taken, if any exist.
    pub oldest: Option<DateTime<Utc>>,
}

/// The result of restoring a path to a checkpoint.
#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    /// The new checkpoint created to record the restore.
    pub checkpoint_id: String,
    /// Paths written back to disk.
    pub restored_paths: Vec<String>,
}

/// The result of undoing a checkpoint's changes.
#[derive(Debug, Clone)]
pub struct UndoOutcome {
    /// The new checkpoint created to record the undo.
    pub checkpoint_id: String,
    /// Paths reverted to their prior content.
    pub reverted_paths: Vec<String>,
    /// Paths that were skipped because they changed again since the
    /// checkpoint being undone, so undoing it would have clobbered a later
    /// edit.
    pub skipped_paths: Vec<String>,
}

/// A page of checkpoint listings.
#[derive(Debug, Clone)]
pub struct CheckpointPage {
    /// The checkpoints on this page, newest first.
    pub items: Vec<CheckpointSummary>,
    /// Opaque cursor for the next page, or `None` on the last page.
    pub next_cursor: Option<String>,
}

/// Errors from the checkpoints engine.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// No checkpoint matches the given id (or id prefix).
    #[error("no checkpoint found matching '{0}'")]
    NotFound(String),
    /// The given path isn't present in the checkpoint's tree.
    #[error("path '{0}' not found at checkpoint '{1}'")]
    PathNotFound(String, String),
    /// A cursor passed to a listing call couldn't be parsed.
    #[error("invalid page cursor")]
    InvalidCursor,
    /// The underlying git object database or refs could not be read or
    /// written.
    #[error("checkpoint repository error: {0}")]
    Git(String),
    /// A filesystem operation (reading a file to snapshot, or writing one
    /// back during a restore) failed.
    #[error("filesystem error: {0}")]
    Io(String),
}
