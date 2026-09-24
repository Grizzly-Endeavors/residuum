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

/// A throwaway checkpoint engine backed by a fresh temp directory, for
/// tests elsewhere in the crate that need a `CheckpointEngine` to satisfy a
/// constructor but don't exercise checkpointing behavior themselves.
#[cfg(test)]
#[must_use]
pub(crate) fn test_engine() -> std::sync::Arc<CheckpointEngine> {
    // Leaked rather than bound to a `TempDir` guard: these callers only
    // need a valid, distinct path for the engine's lifetime, and leaking a
    // handful of empty temp dirs per test run is preferable to plumbing a
    // guard through every one of them.
    let dir = tempfile::tempdir().expect("tempdir").keep();
    std::sync::Arc::new(
        CheckpointEngine::new(
            dir.join("workspace"),
            dir.join("config"),
            &dir.join("checkpoints"),
            None,
        )
        .expect("checkpoint repos should open in a fresh tempdir"),
    )
}
