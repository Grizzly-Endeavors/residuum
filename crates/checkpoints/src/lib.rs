//! Workspace checkpoints: a hidden git history of the workspace and the
//! root config files, used for recovery — not a user-facing version
//! control system. See `docs/systems-usage/checkpoints.md`.

mod backend;
mod engine;
mod exclude;
mod notice;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod types;

pub use engine::{CheckpointEngine, SharedCheckpointRepos, agent_repos_dir, agents_with_history};
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
