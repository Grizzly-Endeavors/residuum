//! Test fixtures for pulse scripts, shared with the gateway's pulse tests.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::script::ScriptEnv;

/// A script environment rooted at `dir`, with no tools `PATH` and agent keys
/// stored under `dir`.
#[must_use]
pub fn script_env(dir: &Path) -> ScriptEnv {
    ScriptEnv {
        workspace_root: dir.to_path_buf(),
        tools_path: Arc::new(RwLock::new(None)),
        agent_keys: residuum_config::agent_keys::AgentKeys::new_shared(dir.join("no-agent-keys")),
    }
}
