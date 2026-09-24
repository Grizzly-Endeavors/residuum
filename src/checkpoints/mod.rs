//! Workspace checkpoints: a hidden git history of the workspace and the
//! root config files, used for recovery — not a user-facing version
//! control system. See `docs/systems-usage/checkpoints.md`.

mod backend;
mod engine;
mod exclude;
mod notice;
mod types;

pub use engine::CheckpointEngine;
pub use types::{
    ChangeKind, ChangedPath, CheckpointContext, CheckpointDetail, CheckpointError, CheckpointPage,
    CheckpointSummary, CheckpointTrigger, RepoKind, RepoStats, RestoreOutcome, UndoOutcome,
};
