//! The context breakdown: the parts of the request a turn sends the model,
//! each with its estimated token count, and the snapshot of it saved after
//! every main turn.

use std::ops::Range;
use std::path::Path;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use residuum_inference::{InferenceProvider, Message, ToolDefinition};
use residuum_memory::tokens::{TOKENS_PER_IMAGE, estimate_request_message_tokens, estimate_tokens};

use super::types::SectionKind;
use crate::turn::ModelRequest;

/// The approximate tokens of each part of the agent's next model request.
/// The parts add up to `total_tokens`, the estimate of the whole request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBreakdown {
    /// Identity and team files in the system prompt: SOUL.md, AGENTS.md,
    /// BOOTSTRAP.md, USER.md, the wiki index and the team roster.
    pub identity_tokens: usize,
    /// The code-owned HARNESS section of the system prompt.
    pub harness_tokens: usize,
    /// The memory pipeline: the observation log and the recent context.
    pub memory_pipeline_tokens: usize,
    /// The skills index.
    pub skills_index_tokens: usize,
    /// Active skill instructions (0 if none are active).
    pub active_skills_tokens: usize,
    /// Built-in tool definitions.
    pub system_tool_tokens: usize,
    /// MCP tool definitions.
    pub mcp_tool_tokens: usize,
    /// The recent message history, as it is sent (sender lines included),
    /// without its status lines.
    pub history_tokens: usize,
    /// Number of messages in the recent history.
    pub history_count: usize,
    /// The status lines stored with the history's messages.
    pub status_line_tokens: usize,
    /// Images the request sends; 0 when the model doesn't take images.
    pub image_count: usize,
    /// Those images, at a flat 1,600 tokens each.
    pub image_tokens: usize,
    /// Earlier reasoning the provider sends back with the history.
    pub reasoning_tokens: usize,
    /// Every part above added up.
    pub total_tokens: usize,
}

impl ContextBreakdown {
    /// The count a system prompt section of `kind` adds to.
    fn section_tokens_mut(&mut self, kind: SectionKind) -> &mut usize {
        match kind {
            SectionKind::Identity => &mut self.identity_tokens,
            SectionKind::Harness => &mut self.harness_tokens,
            SectionKind::Memory => &mut self.memory_pipeline_tokens,
            SectionKind::SkillsIndex => &mut self.skills_index_tokens,
            SectionKind::ActiveSkills => &mut self.active_skills_tokens,
        }
    }

    fn parts_total(&self) -> usize {
        [
            self.identity_tokens,
            self.harness_tokens,
            self.memory_pipeline_tokens,
            self.skills_index_tokens,
            self.active_skills_tokens,
            self.system_tool_tokens,
            self.mcp_tool_tokens,
            self.history_tokens,
            self.status_line_tokens,
            self.image_tokens,
            self.reasoning_tokens,
        ]
        .iter()
        .sum()
    }
}

/// A context breakdown and when it was computed, as the agent saves it after
/// each main turn and the hub serves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    /// When the breakdown was computed (UTC).
    pub computed_at: DateTime<Utc>,
    #[serde(flatten)]
    pub breakdown: ContextBreakdown,
}

/// Count `request`, the request a turn builds, as `provider` sends it: the
/// images only when the model takes them, and the earlier reasoning the
/// provider's own conversion sends back.
pub(crate) fn compute_context_breakdown(
    request: &ModelRequest,
    provider: &dyn InferenceProvider,
) -> ContextBreakdown {
    let reasoning =
        provider.replayed_reasoning(&request.messages, &request.tools, &request.options);
    count_request(request, provider.takes_images(), &reasoning)
}

fn count_request(
    request: &ModelRequest,
    takes_images: bool,
    reasoning: &[String],
) -> ContextBreakdown {
    let layout = &request.layout;
    let mut breakdown = ContextBreakdown::default();

    let conversation_start = match request.messages.first() {
        Some(system) if !layout.system_sections.is_empty() => {
            count_system_prompt(system, &layout.system_sections, &mut breakdown);
            1
        }
        _ => 0,
    };
    for (index, message) in request.messages.iter().enumerate().skip(conversation_start) {
        let tokens = estimate_request_message_tokens(message);
        if layout.status_lines.contains(&index) {
            breakdown.status_line_tokens += tokens;
        } else {
            breakdown.history_tokens += tokens;
            breakdown.history_count += 1;
        }
        if takes_images {
            breakdown.image_count += message.images.len();
        }
    }
    breakdown.image_tokens = breakdown.image_count * TOKENS_PER_IMAGE;

    let (builtin, mcp) = request
        .tools
        .split_at(layout.builtin_tools.min(request.tools.len()));
    breakdown.system_tool_tokens = builtin.iter().map(estimate_tool_tokens).sum();
    breakdown.mcp_tool_tokens = mcp.iter().map(estimate_tool_tokens).sum();
    breakdown.reasoning_tokens = reasoning.iter().map(|text| estimate_tokens(text)).sum();
    breakdown.total_tokens = breakdown.parts_total();
    breakdown
}

/// Share the system message's estimate among its sections. Each section
/// gets the estimate of the text up to its end less what the sections
/// before it got, and the last one gets the rest (the role overhead
/// included), so the shares add up to exactly the message's estimate and
/// each is within a few tokens of its own text's.
fn count_system_prompt(
    system: &Message,
    sections: &[(SectionKind, Range<usize>)],
    breakdown: &mut ContextBreakdown,
) {
    let whole = estimate_request_message_tokens(system);
    let last = sections.len().saturating_sub(1);
    let mut counted = 0;
    for (position, (kind, range)) in sections.iter().enumerate() {
        let through = if position == last {
            whole
        } else {
            system
                .content
                .get(..range.end)
                .map_or(counted, estimate_tokens)
        };
        let share = through.saturating_sub(counted);
        counted += share;
        *breakdown.section_tokens_mut(*kind) += share;
    }
}

/// The estimate of one tool definition as the request sends it.
fn estimate_tool_tokens(definition: &ToolDefinition) -> usize {
    estimate_tokens(&definition.name)
        + estimate_tokens(&definition.description)
        + estimate_tokens(&definition.parameters.to_string())
}

/// Save `snapshot` to `path`, atomically, so a reader never sees half a
/// file. A failure is logged at `warn` and never fails the turn: the hub
/// then serves the previous snapshot until the next turn writes one.
pub(crate) async fn save_context_snapshot(path: &Path, snapshot: &ContextSnapshot) {
    let json = match serde_json::to_string(snapshot) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize the context breakdown");
            return;
        }
    };
    if let Err(e) = residuum_core::util::fs::atomic_write(path, json).await {
        tracing::warn!(
            error = format!("{e:#}"),
            path = %path.display(),
            "failed to save the context breakdown"
        );
    }
}

/// The context snapshot saved at `path`, or `None` when no turn has saved
/// one yet.
///
/// # Errors
/// Returns an error when the file exists but can't be read or parsed.
pub async fn load_context_snapshot(path: &Path) -> anyhow::Result<Option<ContextSnapshot>> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| {
                format!("failed to read the context breakdown at {}", path.display())
            });
        }
    };
    serde_json::from_str(&text).map(Some).with_context(|| {
        format!(
            "failed to parse the context breakdown at {}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use residuum_inference::{
        CompletionOptions, ImageData, InferenceError, InferenceResponse, MessageSender,
        ThinkingBlock, ToolCall,
    };
    use residuum_memory::tokens::estimate_message_tokens;
    use residuum_workspace::identity::IdentityFiles;

    use super::*;
    use crate::Tool;
    use crate::recent_messages::RecentMessages;
    use crate::testing::{EchoTool, RequestInputs, render_model_request};
    use crate::tool::ToolRegistry;

    /// A model that takes images or not and sends `reasoning` back, and is
    /// never called.
    struct Model {
        takes_images: bool,
        reasoning: Vec<String>,
    }

    impl Model {
        fn plain() -> Self {
            Self {
                takes_images: true,
                reasoning: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl InferenceProvider for Model {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            Err(InferenceError::Api("not called".to_string()))
        }

        fn takes_images(&self) -> bool {
            self.takes_images
        }

        fn replayed_reasoning(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Vec<String> {
            self.reasoning.clone()
        }

        fn model_name(&self) -> &'static str {
            "breakdown-test"
        }
    }

    async fn render(inputs: &RequestInputs) -> ModelRequest {
        render_model_request(inputs, &ToolRegistry::new()).await
    }

    async fn breakdown_of(inputs: &RequestInputs, model: &Model) -> ContextBreakdown {
        compute_context_breakdown(&render(inputs).await, model)
    }

    fn history(messages: Vec<Message>) -> RecentMessages {
        let mut recent = RecentMessages::new();
        for message in messages {
            recent.push(message);
        }
        recent
    }

    fn image() -> ImageData {
        ImageData {
            media_type: "image/png".to_string(),
            data: "iVBORw0KGgo=".to_string(),
        }
    }

    /// The estimate of the whole rendered request, taken over the request
    /// itself rather than its parts.
    fn estimate_of(request: &ModelRequest, takes_images: bool, reasoning: &[String]) -> usize {
        let images: usize = if takes_images {
            request.messages.iter().map(|m| m.images.len()).sum()
        } else {
            0
        };
        request
            .messages
            .iter()
            .map(estimate_request_message_tokens)
            .sum::<usize>()
            + request
                .tools
                .iter()
                .map(estimate_tool_tokens)
                .sum::<usize>()
            + images * TOKENS_PER_IMAGE
            + reasoning
                .iter()
                .map(|text| estimate_tokens(text))
                .sum::<usize>()
    }

    #[tokio::test]
    async fn an_empty_agent_counts_only_the_harness() {
        let breakdown = breakdown_of(&RequestInputs::default(), &Model::plain()).await;
        assert!(breakdown.harness_tokens > 0, "HARNESS is always sent");
        assert_eq!(
            breakdown.total_tokens, breakdown.harness_tokens,
            "nothing else is in the request"
        );
        assert_eq!(breakdown.history_count, 0);
    }

    #[tokio::test]
    async fn each_system_prompt_part_rises_by_the_estimate_of_its_section() {
        let base = breakdown_of(&RequestInputs::default(), &Model::plain()).await;
        let soul = "I am a careful assistant who explains things plainly.";
        let observations = "Episode 1: the user asked about rust lifetimes at length.";
        let index = "<available_skills><skill name=\"wiki\"/></available_skills>";
        let active = "<active_skill name=\"wiki\">write pages as facts</active_skill>";
        let inputs = RequestInputs {
            identity: IdentityFiles {
                soul: Some(soul.to_string()),
                ..IdentityFiles::default()
            },
            observations: Some(observations.to_string()),
            skills_index: Some(index.to_string()),
            active_skills: Some(active.to_string()),
            ..RequestInputs::default()
        };
        let breakdown = breakdown_of(&inputs, &Model::plain()).await;

        // A section is counted with its tags and the separator ahead of it;
        // a share can differ from that text's own estimate by rounding.
        let near = |got: usize, text: &str, tag: &str| {
            let expected = estimate_tokens(&format!("\n\n<{tag}>\n{text}\n</{tag}>"));
            assert!(
                got.abs_diff(expected) <= 4,
                "{tag}: counted {got}, its section is about {expected}"
            );
        };
        near(breakdown.identity_tokens, soul, "SOUL.md");
        near(
            breakdown.memory_pipeline_tokens,
            observations,
            "OBSERVATION_LOG",
        );
        near(breakdown.skills_index_tokens, index, "SKILLS_INDEX");
        near(breakdown.active_skills_tokens, active, "ACTIVE_SKILLS");
        assert!(
            breakdown.harness_tokens.abs_diff(base.harness_tokens) <= 4,
            "the harness is the same section either way"
        );
    }

    #[tokio::test]
    async fn the_history_is_counted_as_sent_with_sender_lines() {
        let sender = MessageSender {
            name: "Jane".to_string(),
            id: "id-jane".to_string(),
            interface: "teams".to_string(),
            location: Some("#eng".to_string()),
        };
        let attributed = Message::user("can you check the build?").with_sender(Some(sender));
        let sent = Message::user(attributed.attributed_content().into_owned());
        let inputs = RequestInputs {
            recent_messages: history(vec![attributed, Message::assistant("on it", None)]),
            ..RequestInputs::default()
        };
        let breakdown = breakdown_of(&inputs, &Model::plain()).await;
        assert_eq!(breakdown.history_count, 2);
        assert_eq!(
            breakdown.history_tokens,
            estimate_request_message_tokens(&sent)
                + estimate_request_message_tokens(&Message::assistant("on it", None)),
            "the sender line the model sees is counted"
        );
    }

    #[tokio::test]
    async fn each_stored_status_line_is_counted_apart_from_the_history() {
        let mut stamped = Message::user("hi");
        stamped.status_line = Some("[Received: Mon 2026-11-02 09:00 UTC]".to_string());
        let line = Message::system(stamped.status_line.clone().unwrap());
        let plain = RequestInputs {
            recent_messages: history(vec![Message::user("hi")]),
            ..RequestInputs::default()
        };
        let with_line = RequestInputs {
            recent_messages: history(vec![stamped]),
            ..RequestInputs::default()
        };

        let before = breakdown_of(&plain, &Model::plain()).await;
        let after = breakdown_of(&with_line, &Model::plain()).await;

        assert_eq!(before.status_line_tokens, 0);
        assert_eq!(
            after.status_line_tokens,
            estimate_request_message_tokens(&line),
            "the line is counted as the system message it is sent as"
        );
        assert_eq!(after.history_tokens, before.history_tokens);
        assert_eq!(
            after.history_count, 1,
            "a status line is not a message of the history"
        );
        assert_eq!(
            after.total_tokens,
            before.total_tokens + after.status_line_tokens
        );
    }

    #[tokio::test]
    async fn each_image_counts_a_flat_amount_when_the_model_takes_images() {
        let text_only_history = || history(vec![Message::user("look")]);
        let with_images = || {
            history(vec![Message::user_with_images(
                "look",
                vec![image(), image()],
            )])
        };
        let without = breakdown_of(
            &RequestInputs {
                recent_messages: text_only_history(),
                ..RequestInputs::default()
            },
            &Model::plain(),
        )
        .await;
        let inputs = RequestInputs {
            recent_messages: with_images(),
            ..RequestInputs::default()
        };
        let taken = breakdown_of(&inputs, &Model::plain()).await;
        let refused = breakdown_of(
            &inputs,
            &Model {
                takes_images: false,
                reasoning: Vec::new(),
            },
        )
        .await;

        assert_eq!(taken.image_count, 2);
        assert_eq!(taken.image_tokens, 2 * TOKENS_PER_IMAGE);
        assert_eq!(
            taken.total_tokens,
            without.total_tokens + 2 * TOKENS_PER_IMAGE,
            "the images add exactly their flat count"
        );
        assert_eq!(refused.image_count, 0, "a text-only model is sent none");
        assert_eq!(refused.image_tokens, 0);
    }

    #[tokio::test]
    async fn each_tool_list_is_counted_separately() {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(EchoTool));
        let registry = residuum_mcp::McpRegistry::new_shared();
        let mcp_tool = ToolDefinition {
            name: "search".to_string(),
            description: "Search the docs".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        registry
            .write()
            .await
            .insert_running_server("docs", vec![mcp_tool]);
        let inputs = RequestInputs {
            mcp: Some(Arc::clone(&registry)),
            ..RequestInputs::default()
        };
        let request = render_model_request(&inputs, &tools).await;
        let breakdown = compute_context_breakdown(&request, &Model::plain());

        assert_eq!(request.layout.builtin_tools, 1);
        assert_eq!(request.tools.len(), 2, "the echo tool, then the MCP tool");
        assert_eq!(
            breakdown.system_tool_tokens,
            estimate_tool_tokens(&EchoTool.definition())
        );
        assert_eq!(
            breakdown.mcp_tool_tokens,
            estimate_tool_tokens(request.tools.get(1).expect("the MCP tool is second"))
        );
    }

    #[tokio::test]
    async fn reasoning_is_counted_only_as_the_provider_sends_it_back() {
        let inputs = RequestInputs {
            recent_messages: history(vec![Message::user("hi")]),
            ..RequestInputs::default()
        };
        let none = breakdown_of(&inputs, &Model::plain()).await;
        let replayed = breakdown_of(
            &inputs,
            &Model {
                takes_images: true,
                reasoning: vec!["first I'll check the logs".to_string(), "then".to_string()],
            },
        )
        .await;

        assert_eq!(none.reasoning_tokens, 0, "no reasoning goes back");
        let expected = estimate_tokens("first I'll check the logs") + estimate_tokens("then");
        assert_eq!(replayed.reasoning_tokens, expected);
        assert_eq!(replayed.total_tokens, none.total_tokens + expected);
    }

    #[tokio::test]
    async fn the_total_is_the_estimate_of_the_whole_rendered_request() {
        let mut stamped = Message::user_with_images("what's this?", vec![image()]).with_sender(
            Some(MessageSender {
                name: "Sam".to_string(),
                id: "id-sam".to_string(),
                interface: "discord".to_string(),
                location: None,
            }),
        );
        stamped.status_line = Some("[Received: Mon 9:00]".to_string());
        let call = Message::assistant(
            "",
            Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({"text": "hello"}),
                server: None,
            }]),
        );
        let inputs = RequestInputs {
            identity: IdentityFiles {
                soul: Some("soul".to_string()),
                user: Some("The user writes Rust.".to_string()),
                team: Some("You are \"a\".".to_string()),
                ..IdentityFiles::default()
            },
            recent_messages: history(vec![
                stamped,
                call,
                Message::tool("echoed", "call_1"),
                Message::assistant("It's a cat.", None),
                Message::system("background note"),
            ]),
            observations: Some("obs".to_string()),
            recent_context: Some("recent".to_string()),
            skills_index: Some("<available_skills/>".to_string()),
            active_skills: Some("<active/>".to_string()),
            ..RequestInputs::default()
        };
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(EchoTool));
        let request = render_model_request(&inputs, &tools).await;
        let reasoning = vec!["reasoning sent back".to_string()];
        let model = Model {
            takes_images: true,
            reasoning: reasoning.clone(),
        };

        let breakdown = compute_context_breakdown(&request, &model);

        assert_eq!(
            breakdown.total_tokens,
            estimate_of(&request, true, &reasoning),
            "every part of the request is counted, once"
        );
        assert_eq!(breakdown.total_tokens, breakdown.parts_total());
        assert_eq!(breakdown.history_count, 5);
        assert_eq!(breakdown.image_count, 1);
    }

    #[tokio::test]
    async fn a_stored_message_is_counted_once_across_the_parts_it_is_sent_as() {
        let mut asked = Message::user_with_images("what's this?", vec![image()]).with_sender(Some(
            MessageSender {
                name: "Sam".to_string(),
                id: "id-sam".to_string(),
                interface: "discord".to_string(),
                location: None,
            },
        ));
        asked.status_line = Some("[Received: Mon 2026-11-02 09:00 UTC]".to_string());
        let answered =
            Message::assistant("It's a cat.", None).with_thinking(vec![ThinkingBlock::text(
                "it has whiskers and pointed ears",
            )]);
        let stored = vec![asked, answered];
        let inputs = RequestInputs {
            recent_messages: history(stored.clone()),
            ..RequestInputs::default()
        };
        // A provider that takes the image and sends all the reasoning back.
        let model = Model {
            takes_images: true,
            reasoning: vec!["it has whiskers and pointed ears".to_string()],
        };

        let breakdown = breakdown_of(&inputs, &model).await;

        // The stored history's own estimate counts the sender line, status
        // line, image and reasoning with each message; the breakdown counts
        // each in its own part. They differ only by each part rounding up on
        // its own, while counting any of them twice adds a whole line or
        // image.
        let sent = breakdown.history_tokens
            + breakdown.status_line_tokens
            + breakdown.image_tokens
            + breakdown.reasoning_tokens;
        let stored_estimate = estimate_message_tokens(&stored);
        assert!(
            sent.abs_diff(stored_estimate) <= 3,
            "the request's parts count {sent}, the stored history {stored_estimate}"
        );
    }

    #[tokio::test]
    async fn a_snapshot_round_trips_and_is_absent_until_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context_breakdown.json");
        assert_eq!(load_context_snapshot(&path).await.unwrap(), None);

        let snapshot = ContextSnapshot {
            computed_at: Utc::now(),
            breakdown: breakdown_of(&RequestInputs::default(), &Model::plain()).await,
        };
        save_context_snapshot(&path, &snapshot).await;

        assert_eq!(load_context_snapshot(&path).await.unwrap(), Some(snapshot));
        let json: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(&path).await.unwrap()).unwrap();
        assert!(
            json.get("computed_at").is_some() && json.get("total_tokens").is_some(),
            "the breakdown's fields sit beside the time it was computed: {json}"
        );
    }

    #[tokio::test]
    async fn an_unreadable_snapshot_is_an_error_not_an_absence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context_breakdown.json");
        tokio::fs::write(&path, "not json").await.unwrap();
        assert!(load_context_snapshot(&path).await.is_err());
    }
}
