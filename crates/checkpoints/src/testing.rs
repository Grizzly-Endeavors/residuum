use std::sync::Arc;

use crate::CheckpointEngine;

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
