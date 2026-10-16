//! Tool system for agent-invoked operations.

pub(crate) mod a2a_task_update;
pub mod actions;
mod agent_keys;
pub mod agent_lifecycle;
pub mod background;
pub mod config_reload_tracker;
mod edit;
mod exec;
pub(crate) mod file_bug_report;
pub(crate) mod file_tracker;
pub mod inbox;
pub(crate) mod list_conversations;
pub(crate) mod list_endpoints;
pub mod memory_get;
pub mod memory_search;
pub mod message_agent;
pub(crate) mod ollama_web_search;
pub mod path_policy;
mod read;
pub mod registration;
mod registry;
pub mod send_message;
pub mod skills;
pub(crate) mod submit_feedback;
pub(crate) mod switch_endpoint;
#[cfg(test)]
pub(crate) mod team_namespace_tests;
pub(crate) mod web_fetch;
pub mod workspace_checkpoints;
mod write;

pub use agent_lifecycle::LifecycleAccess;
pub use config_reload_tracker::{ConfigWriteWatch, SharedConfigReloadTracker};
pub use file_tracker::{FileTracker, SharedFileTracker};
pub use path_policy::{PathPolicy, SharedPathPolicy};
pub use registry::{SubagentToolDeps, build_subagent_registry};

use serde_json::Value;

use crate::inference::ToolError;

pub(super) fn require_str<'a>(args: &'a Value, field: &'static str) -> Result<&'a str, ToolError> {
    args.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments(format!("{field} is required")))
}

/// Publish a plain-language notice to the system notification channel, if a
/// publisher is configured. Never fails the caller: a publish error is
/// logged and otherwise ignored. Shared by tools that take user-visible
/// action on the user's behalf (e.g. overwriting or deleting a key the user
/// created) and need to surface it outside the transcript.
pub(super) async fn publish_notice(publisher: Option<&crate::bus::Publisher>, message: String) {
    if let Some(publisher) = publisher {
        crate::bus::notice::publish_notice(publisher, message).await;
    }
}
