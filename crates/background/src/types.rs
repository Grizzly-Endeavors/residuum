use std::sync::Arc;

use residuum_agent::HopCounter;
use residuum_bus::MessageEvent;
use residuum_bus::SessionAddress;
use residuum_bus::session::SessionCategory;
use residuum_config::ModelSelector;
use residuum_config::WorkspaceLayout;
use residuum_inference::{ImageData, MessageSender};
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::Observer;
use residuum_workspace::identity::IdentityFiles;

use super::session_tools::SessionToolFactory;

#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    pub prompt: String,
    pub context: Option<String>,
    pub model: ModelSelector,
    pub resolved_model: Option<String>,
    pub hop_count: u32,
    pub sender: Option<MessageSender>,
    pub inbound: Option<MessageEvent>,
    pub images: Vec<ImageData>,
    pub carried_history: Vec<residuum_inference::Message>,
    pub delivered_messages: Vec<residuum_bus::AgentMessageEvent>,
    pub delivered_chat_messages: Vec<MessageEvent>,
}

impl SubAgentConfig {
    pub(crate) fn opening_text(&self) -> String {
        self.delivered_messages
            .iter()
            .map(residuum_bus::AgentMessageEvent::format_for_agent)
            .chain((!self.prompt.is_empty()).then(|| self.prompt.clone()))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[must_use]
pub(crate) fn truncate_prompt_preview(prompt: &str) -> String {
    prompt.chars().take(120).collect()
}

pub struct SubAgentBuildConfig {
    pub workspace_layout: WorkspaceLayout,
    pub identity: IdentityFiles,
    pub options: residuum_inference::CompletionOptions,
    pub max_tool_iterations: Option<usize>,
    pub repeat_call_guard: residuum_config::RepeatCallGuardConfig,
    pub intermediate_delivery: residuum_agent::context::IntermediateDelivery,
    pub skill: Option<String>,
    pub observations: Option<String>,
    pub recent_context: Option<String>,
    pub observer: Arc<Observer>,
    pub merge_writer: Arc<MemoryMergeWriter>,
    pub episode_skip_token_floor: usize,
    pub session_tools: Arc<dyn SessionToolFactory>,
    pub own_address: SessionAddress,
    pub own_depth: u32,
    pub session_category: SessionCategory,
    pub trigger: residuum_bus::EventTrigger,
    pub conversation_target: Option<residuum_bus::ConversationTarget>,
    pub hop_counter: HopCounter,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_tier_default_is_medium() {
        assert_eq!(
            ModelSelector::default(),
            ModelSelector::MEDIUM,
            "default tier should be medium"
        );
    }

    #[test]
    fn truncate_prompt_preview_truncates_at_120_chars() {
        let long_prompt = "x".repeat(200);
        let preview = truncate_prompt_preview(&long_prompt);
        assert_eq!(preview.len(), 120, "preview should be capped at 120 chars");
    }
}
