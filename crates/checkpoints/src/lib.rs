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

#[must_use]
pub fn is_root_relative(path: &str) -> bool {
    !path.is_empty()
        && std::path::Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
