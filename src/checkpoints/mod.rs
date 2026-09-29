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

/// Whether `path` names something inside a checkpointed root: only plain
/// relative components, so it can't escape the root it's joined onto. A
/// rooted path like `/etc/passwd` isn't `is_absolute()` on Windows (no
/// drive), so each component is checked rather than the whole path.
#[must_use]
pub fn is_root_relative(path: &str) -> bool {
    !path.is_empty()
        && std::path::Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// A throwaway checkpoint engine backed by a fresh temp directory, for
/// tests elsewhere in the crate that need a `CheckpointEngine` to satisfy a
/// constructor but don't exercise checkpointing behavior themselves. The
/// temp directory's guard lives inside the returned engine, so it's cleaned
/// up when the last `Arc` reference to the engine drops.
#[cfg(test)]
#[must_use]
pub(crate) fn test_engine() -> std::sync::Arc<CheckpointEngine> {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = CheckpointEngine::new(
        dir.path().join("workspace"),
        dir.path().join("agent-config"),
        dir.path().join("hub"),
        &dir.path().join("checkpoints"),
        None,
    )
    .expect("checkpoint repos should open in a fresh tempdir")
    .with_tempdir_guard(dir);
    std::sync::Arc::new(engine)
}
