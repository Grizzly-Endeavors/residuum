//! Agent key discovery and cleanup tools: `agent_keys_list`, `agent_key_delete`.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult, require_str};
use crate::agent_keys::{AgentKeyError, KeyCreator, SharedAgentKeys};
use crate::checkpoints::{CheckpointContext, CheckpointEngine, CheckpointTrigger};
use crate::inference::ToolDefinition;

/// Lists agent keys: names, environment variables, creators, descriptions.
pub struct AgentKeysListTool {
    keys: SharedAgentKeys,
}

impl AgentKeysListTool {
    #[must_use]
    pub fn new(keys: SharedAgentKeys) -> Self {
        Self { keys }
    }
}

#[async_trait]
impl Tool for AgentKeysListTool {
    fn name(&self) -> &'static str {
        "agent_keys_list"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "List the agent keys (API keys, tokens) available to exec. Shows each \
                          key's name, the environment variable it is exposed as, who created it, \
                          and its description. Values are never shown; use a key by naming it in \
                          exec's `keys` parameter."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
        let snapshot = match self.keys.snapshot().await {
            Ok(s) => s,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "couldn't read the agent key store: {e}"
                )));
            }
        };
        let keys = snapshot.store.list();
        if keys.is_empty() {
            return Ok(ToolResult::success(
                "no agent keys stored. The user can add one with `residuum agent-keys set <name>` \
                 or in the web UI settings under Agent keys; you can mint one with exec's \
                 `store_output_as`.",
            ));
        }

        let mut out = format!(
            "{} agent key(s). Expose one to a command with exec's `keys` parameter; values are \
             redacted from all output.\n",
            keys.len()
        );
        for key in keys {
            let description = if key.description.is_empty() {
                "(no description)"
            } else {
                key.description.as_str()
            };
            _ = writeln!(
                out,
                "- {} -> ${} (created by {}): {}",
                key.name,
                key.env_var,
                key.created_by.as_str(),
                description
            );
        }
        Ok(ToolResult::success(out.trim_end()))
    }
}

/// Deletes an agent key the agent itself created.
pub struct AgentKeyDeleteTool {
    keys: SharedAgentKeys,
    checkpoints: Arc<CheckpointEngine>,
}

impl AgentKeyDeleteTool {
    #[must_use]
    pub fn new(keys: SharedAgentKeys, checkpoints: Arc<CheckpointEngine>) -> Self {
        Self { keys, checkpoints }
    }
}

#[async_trait]
impl Tool for AgentKeyDeleteTool {
    fn name(&self) -> &'static str {
        "agent_key_delete"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Delete an agent key you created (e.g. a minted token that is no \
                          longer needed). Keys the user created can't be deleted this way."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the agent key to delete"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = require_str(&arguments, "name")?;
        self.checkpoints
            .checkpoint_config_before_write(CheckpointContext::system(
                CheckpointTrigger::PreConfigWrite,
                format!("agent deleted agent key '{name}'"),
            ))
            .await;
        match self.keys.delete(name, KeyCreator::Agent).await {
            Ok(()) => Ok(ToolResult::success(format!("deleted agent key '{name}'"))),
            Err(e @ (AgentKeyError::NotFound(_) | AgentKeyError::OwnedByUser(_))) => {
                Ok(ToolResult::error(e.to_string()))
            }
            Err(e) => {
                tracing::warn!(error = %e, key = %name, "failed to delete agent key");
                Ok(ToolResult::error(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_keys::AgentKeys;

    #[tokio::test]
    async fn list_shows_metadata_never_values() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new_shared(dir.path());
        keys.set(
            "github_token",
            "ghp_supersecret99",
            Some("repo access"),
            KeyCreator::User,
        )
        .await
        .unwrap();

        let result = AgentKeysListTool::new(keys)
            .execute(serde_json::json!({}))
            .await
            .unwrap();
        assert!(!result.is_error, "listing should succeed");
        assert!(
            result
                .output
                .contains("github_token -> $GITHUB_TOKEN (created by user): repo access"),
            "should describe the key: {}",
            result.output
        );
        assert!(
            !result.output.contains("ghp_supersecret99"),
            "must never show the value"
        );
    }

    #[tokio::test]
    async fn list_empty_explains_how_to_add() {
        let dir = tempfile::tempdir().unwrap();
        let result = AgentKeysListTool::new(AgentKeys::new_shared(dir.path()))
            .execute(serde_json::json!({}))
            .await
            .unwrap();
        assert!(
            result.output.contains("residuum agent-keys set"),
            "empty listing should say how keys get added: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn delete_refuses_user_keys_and_removes_agent_keys() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new_shared(dir.path());
        keys.set("users", "user-value-123", None, KeyCreator::User)
            .await
            .unwrap();
        keys.set("minted", "agent-value-123", None, KeyCreator::Agent)
            .await
            .unwrap();
        let tool = AgentKeyDeleteTool::new(
            std::sync::Arc::clone(&keys),
            crate::checkpoints::test_engine(),
        );

        let refused = tool
            .execute(serde_json::json!({ "name": "users" }))
            .await
            .unwrap();
        assert!(refused.is_error, "user key delete should be refused");
        assert!(
            refused.output.contains("created by the user"),
            "refusal should explain why: {}",
            refused.output
        );

        let deleted = tool
            .execute(serde_json::json!({ "name": "minted" }))
            .await
            .unwrap();
        assert!(!deleted.is_error, "agent key delete should succeed");
        let snap = keys.snapshot().await.unwrap();
        assert!(snap.store.value("minted").is_none(), "minted key gone");
        assert!(snap.store.value("users").is_some(), "user key kept");
    }

    #[tokio::test]
    async fn delete_checkpoints_the_config_repo_first() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new_shared(dir.path());
        keys.set("minted", "agent-value-123", None, KeyCreator::Agent)
            .await
            .unwrap();
        let engine = Arc::new(
            CheckpointEngine::new(
                dir.path().join("workspace"),
                dir.path().to_path_buf(),
                &dir.path().join("checkpoints"),
                None,
            )
            .unwrap(),
        );
        let tool = AgentKeyDeleteTool::new(keys, Arc::clone(&engine));

        let result = tool
            .execute(serde_json::json!({ "name": "minted" }))
            .await
            .unwrap();
        assert!(!result.is_error, "delete should succeed: {}", result.output);

        let page = engine
            .list_checkpoints(crate::checkpoints::RepoKind::Config, None, None, None)
            .await
            .unwrap();
        assert_eq!(
            page.items.len(),
            1,
            "deleting an agent key should checkpoint the config repo before the write"
        );
        assert_eq!(
            page.items.first().unwrap().trigger,
            CheckpointTrigger::PreConfigWrite
        );
    }
}
