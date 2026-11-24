#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;

#[cfg(test)]
mod a2a_client_e2e_tests;
pub(crate) mod a2a_task_update;
pub mod actions;
mod agent_keys;
pub mod agent_lifecycle;
pub mod background;
pub mod config_reload_tracker;
#[cfg(test)]
mod default_request_tests;
mod edit;
mod exec;
pub(crate) mod file_bug_report;
pub(crate) mod file_tracker;
pub mod inbox;
pub(crate) mod list_conversations;
pub(crate) mod list_endpoints;
pub mod main_tools;
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
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub(crate) mod web_fetch;
pub mod workspace_checkpoints;
mod write;

pub use agent_lifecycle::LifecycleAccess;
pub use config_reload_tracker::{ConfigWriteWatch, SharedConfigReloadTracker};
pub use file_tracker::{FileTracker, SharedFileTracker};
pub use path_policy::{PathPolicy, SharedPathPolicy};
pub use registry::SessionTools;

use serde_json::Value;

use residuum_inference::ToolError;

pub(crate) const MODEL_PARAMETER_DESCRIPTION: &str = "A tier (\"small\", \"medium\", or \"large\") or an explicit model id in provider/model form (the format providers.toml uses). An explicit model runs that one model with no failover. Defaults to \"medium\".";

pub(crate) const PAGE_BUDGET: usize = residuum_agent::HISTORY_RESULT_MAX_CHARS - 2_000;

pub(crate) fn require_str<'a>(args: &'a Value, field: &'static str) -> Result<&'a str, ToolError> {
    args.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments(format!("{field} is required")))
}

pub(crate) fn split_chars(text: &str, max: usize) -> (&str, usize) {
    match text.char_indices().nth(max) {
        None => (text, 0),
        Some((byte, _)) => {
            let (head, tail) = text.split_at(byte);
            (head, tail.chars().count())
        }
    }
}

pub(crate) async fn publish_notice(publisher: Option<&residuum_bus::Publisher>, message: String) {
    if let Some(publisher) = publisher {
        residuum_bus::notice::publish_notice(publisher, message).await;
    }
}

#[cfg(test)]
mod tests {
    use super::split_chars;

    #[test]
    fn split_chars_counts_characters_not_bytes() {
        assert_eq!(split_chars("héllo", 2), ("hé", 3));
        assert_eq!(split_chars("héllo", 5), ("héllo", 0));
        assert_eq!(split_chars("héllo", 99), ("héllo", 0));
        assert_eq!(split_chars("", 3), ("", 0));
    }
}
