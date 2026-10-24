//! Test fixtures for code that needs a checkpoint engine. Behind
//! `test-support`, and compiled for this crate's own tests.

use std::sync::Arc;

use crate::CheckpointEngine;

/// A throwaway checkpoint engine backed by a fresh temp directory, for
/// tests that need a `CheckpointEngine` to satisfy a constructor but don't
/// exercise checkpointing behavior themselves. The temp directory's guard
/// lives inside the returned engine, so it's cleaned up when the last `Arc`
/// reference to the engine drops.
///
/// # Panics
///
/// Panics when the temporary directory or the checkpoint repositories inside
/// it cannot be created.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "test-support helper: a fixture that cannot open its repositories fails the test that asked for it"
)]
pub fn test_engine() -> Arc<CheckpointEngine> {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = CheckpointEngine::new(
        "test-agent",
        dir.path().join("workspace"),
        &residuum_config::paths::TeamPaths::new(dir.path().join("team")),
        dir.path().join("agent-config"),
        dir.path().join("hub"),
        &dir.path().join("checkpoints"),
        None,
    )
    .expect("checkpoint repos should open in a fresh tempdir")
    .with_tempdir_guard(dir);
    Arc::new(engine)
}
