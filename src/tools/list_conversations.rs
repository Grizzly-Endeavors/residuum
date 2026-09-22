//! List the conversations the agent can post into on each chat interface.

use async_trait::async_trait;
use serde_json::Value;

use crate::bus::EndpointRegistry;
use crate::inference::ToolDefinition;
use crate::interfaces::types::ConversationKind;

use super::{Tool, ToolError, ToolResult};

/// Tool for listing reachable DMs, group chats, and channels per chat interface.
pub struct ListConversationsTool {
    registry: EndpointRegistry,
}

impl ListConversationsTool {
    /// Create a new `ListConversationsTool`.
    #[must_use]
    pub fn new(registry: EndpointRegistry) -> Self {
        Self { registry }
    }
}

fn kind_label(kind: ConversationKind) -> &'static str {
    match kind {
        ConversationKind::Personal => "direct message",
        ConversationKind::GroupChat => "group chat",
        ConversationKind::Channel => "channel",
    }
}

#[async_trait]
impl Tool for ListConversationsTool {
    fn name(&self) -> &'static str {
        "list_conversations"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "List the direct messages, group chats, and channels you can post \
                into on each chat interface (discord, telegram, teams). Pass an ID from here \
                as send_message's 'conversation' to post there. Chats appear once the bot has \
                been added to them or has heard from them; Discord server channels are listed \
                directly."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "endpoint": {
                        "type": "string",
                        "description": "Only list conversations on this chat endpoint (e.g. \"teams\")"
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let directory = self.registry.conversations();
        let mut endpoints = directory.endpoints();
        endpoints.sort();
        if let Some(only) = arguments.get("endpoint").and_then(Value::as_str) {
            if !endpoints.iter().any(|e| e == only) {
                return Ok(ToolResult::error(format!(
                    "'{only}' is not a running chat interface; running: {}",
                    if endpoints.is_empty() {
                        "(none)".to_string()
                    } else {
                        endpoints.join(", ")
                    }
                )));
            }
            endpoints.retain(|e| e == only);
        }
        if endpoints.is_empty() {
            return Ok(ToolResult::success(
                "No chat interfaces are running (discord, telegram, and teams list conversations).",
            ));
        }

        let mut sections = Vec::new();
        for endpoint in endpoints {
            let Some(source) = directory.source(&endpoint) else {
                continue;
            };
            let mut lines = vec![format!("{endpoint}:")];
            match source.conversations().await {
                Ok(mut conversations) if !conversations.is_empty() => {
                    conversations.sort_by(|a, b| a.label.cmp(&b.label));
                    lines.extend(conversations.iter().map(|c| {
                        format!("  {} \u{2014} {} ({})", c.id, c.label, kind_label(c.kind))
                    }));
                }
                Ok(_) => lines.push("  (none yet)".to_string()),
                Err(e) => {
                    tracing::warn!(error = %e, endpoint = %endpoint, "failed to list conversations");
                    lines.push(format!("  couldn't list conversations: {e}"));
                }
            }
            sections.push(lines.join("\n"));
        }
        Ok(ToolResult::success(sections.join("\n\n")))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::interfaces::conversations::{ConversationSource, KnownConversation};

    struct Fixed(anyhow::Result<Vec<KnownConversation>, String>);

    #[async_trait]
    impl ConversationSource for Fixed {
        async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>> {
            self.0.clone().map_err(anyhow::Error::msg)
        }
    }

    fn convo(id: &str, kind: ConversationKind, label: &str) -> KnownConversation {
        KnownConversation {
            id: id.to_string(),
            kind,
            label: label.to_string(),
        }
    }

    #[tokio::test]
    async fn lists_each_running_interface() {
        let registry = EndpointRegistry::default();
        let _teams = registry.conversations().register(
            "teams",
            Arc::new(Fixed(Ok(vec![
                convo("19:b", ConversationKind::Channel, "#builds (Eng)"),
                convo(
                    "a:1",
                    ConversationKind::Personal,
                    "direct message with Bear",
                ),
            ]))),
        );
        let _discord = registry.conversations().register(
            "discord",
            Arc::new(Fixed(Err("discord is down".to_string()))),
        );
        let tool = ListConversationsTool::new(registry);

        let out = tool.execute(serde_json::json!({})).await.unwrap().output;
        assert!(
            out.contains("discord:\n  couldn't list conversations: discord is down"),
            "{out}"
        );
        assert!(
            out.contains("  19:b \u{2014} #builds (Eng) (channel)"),
            "{out}"
        );
        assert!(
            out.contains("  a:1 \u{2014} direct message with Bear (direct message)"),
            "{out}"
        );

        let only = tool
            .execute(serde_json::json!({"endpoint": "teams"}))
            .await
            .unwrap()
            .output;
        assert!(!only.contains("discord"), "{only}");
    }

    #[tokio::test]
    async fn unknown_endpoint_filter_is_an_error() {
        let tool = ListConversationsTool::new(EndpointRegistry::default());
        let result = tool
            .execute(serde_json::json!({"endpoint": "slack"}))
            .await
            .unwrap();
        assert!(result.is_error);
    }
}
