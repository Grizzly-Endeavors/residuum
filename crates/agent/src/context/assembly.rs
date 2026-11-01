//! System prompt assembly: combines identity, memory, and context into messages.

use residuum_inference::Message;
use residuum_memory::tokens::{estimate_message_tokens, estimate_tokens};
use residuum_workspace::identity::IdentityFiles;

use super::super::recent_messages::RecentMessages;
use super::prompt::build_system_content;
use super::types::{ContextBreakdown, MemoryContext, PromptContext};

/// Compute a per-section token breakdown for the current agent context.
pub(crate) fn compute_context_breakdown(
    identity: &IdentityFiles,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    recent_messages: &RecentMessages,
    system_tool_tokens: usize,
    mcp_tool_tokens: usize,
) -> ContextBreakdown {
    let identity_tokens = [
        identity.soul.as_deref(),
        identity.agents.as_deref(),
        identity.bootstrap.as_deref(),
        identity.user.as_deref(),
        identity.wiki_index.as_deref(),
        identity.team.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(estimate_tokens)
    .sum();

    let memory_pipeline_tokens = memory_ctx.observations.map_or(0, estimate_tokens)
        + memory_ctx.recent_context.map_or(0, estimate_tokens);

    let msgs = recent_messages.messages();

    ContextBreakdown {
        identity_tokens,
        memory_pipeline_tokens,
        skills_index_tokens: prompt_ctx.skills.index.map_or(0, estimate_tokens),
        active_skills_tokens: prompt_ctx
            .skills
            .active_instructions
            .map_or(0, estimate_tokens),
        system_tool_tokens,
        mcp_tool_tokens,
        history_tokens: estimate_message_tokens(msgs),
        history_count: msgs.len(),
    }
}

/// Assemble the full message list for a model call.
///
/// Creates a system message from identity files and observation log content,
/// then appends the recent messages. A message that carries a status line
/// ([`Message::status_line`]) is preceded by it as a system message, the
/// bytes stored when the message arrived, so earlier turns replay unchanged.
#[must_use]
pub(crate) fn assemble_system_prompt(
    identity: &IdentityFiles,
    recent_messages: &RecentMessages,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
) -> Vec<Message> {
    let system_content = build_system_content(identity, memory_ctx, &prompt_ctx.skills);

    let conversation = recent_messages.messages();
    let status_lines = conversation
        .iter()
        .filter(|m| m.status_line.is_some())
        .count();
    let mut messages = Vec::with_capacity(1 + conversation.len() + status_lines);

    messages.push(Message::system(system_content));
    for m in conversation {
        if let Some(line) = &m.status_line {
            messages.push(Message::system(line.clone()));
        }
        let mut rendered = m.clone();
        if m.sender.is_some() {
            rendered.content = m.attributed_content().into_owned();
        }
        messages.push(rendered);
    }

    messages
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;
    use crate::context::types::SkillsContext;
    use residuum_inference::Role;

    fn no_memory() -> MemoryContext<'static> {
        MemoryContext {
            observations: None,
            recent_context: None,
        }
    }

    #[test]
    fn assemble_with_empty_identity() {
        let identity = IdentityFiles::default();
        let recent = RecentMessages::new();

        let messages =
            assemble_system_prompt(&identity, &recent, &no_memory(), &PromptContext::default());
        assert_eq!(messages.len(), 1, "should have system message only");
        assert_eq!(
            messages.first().map(|m| &m.role),
            Some(&Role::System),
            "first message should be system"
        );
        assert!(
            messages[0].content.contains("<HARNESS>"),
            "default identity should still carry the code-owned HARNESS section"
        );
        assert!(
            !messages[0].content.contains("<SOUL.md>"),
            "no identity sections should render for default identity"
        );
    }

    #[test]
    fn assemble_includes_message_history() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("hello"));

        let messages =
            assemble_system_prompt(&identity, &recent, &no_memory(), &PromptContext::default());
        assert_eq!(messages.len(), 2, "should have system + user message");
        assert_eq!(
            messages[0].role,
            Role::System,
            "first message should be system"
        );
        assert_eq!(
            messages[1].role,
            Role::User,
            "second message should be user"
        );
        assert_eq!(
            messages[1].content, "hello",
            "user message content should match"
        );
    }

    #[test]
    fn assemble_attributes_every_user_message_with_a_sender() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        let sender = |name: &str| residuum_inference::MessageSender {
            name: name.to_string(),
            id: format!("id-{name}"),
            interface: "teams".to_string(),
            location: Some("#eng".to_string()),
        };
        recent.push(Message::user("first").with_sender(Some(sender("Jane"))));
        recent.push(Message::assistant("ok", None));
        recent.push(Message::user("second").with_sender(Some(sender("Sam"))));
        recent.push(Message::user("from the web ui"));

        let messages =
            assemble_system_prompt(&identity, &recent, &no_memory(), &PromptContext::default());
        let contents: Vec<&str> = messages[1..].iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            contents,
            [
                "[From: Jane via teams (#eng)]\nfirst",
                "ok",
                "[From: Sam via teams (#eng)]\nsecond",
                "from the web ui",
            ],
            "earlier senders stay attributed, not just the latest message"
        );
    }

    fn stamped_user(content: &str, line: &str) -> Message {
        let mut message = Message::user(content);
        message.status_line = Some(line.to_string());
        message
    }

    #[test]
    fn stored_status_lines_precede_their_messages() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(stamped_user("first", "[Received: one]"));
        recent.push(Message::assistant("response", None));
        recent.push(Message::system("background note"));
        recent.push(stamped_user("second", "[Received: two]"));

        let messages =
            assemble_system_prompt(&identity, &recent, &no_memory(), &PromptContext::default());

        let rendered: Vec<(Role, &str)> = messages[1..]
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            rendered,
            [
                (Role::System, "[Received: one]"),
                (Role::User, "first"),
                (Role::Assistant, "response"),
                (Role::System, "background note"),
                (Role::System, "[Received: two]"),
                (Role::User, "second"),
            ],
            "each stored line sits immediately before its own message"
        );
    }

    #[test]
    fn messages_without_a_stored_line_get_none() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("hello"));

        let messages =
            assemble_system_prompt(&identity, &recent, &no_memory(), &PromptContext::default());
        assert_eq!(messages.len(), 2, "system prompt + user, no status line");
    }

    #[test]
    fn context_breakdown_empty_identity_no_messages() {
        let identity = IdentityFiles::default();
        let memory = no_memory();
        let recent = RecentMessages::new();

        let bd =
            compute_context_breakdown(&identity, &memory, &PromptContext::default(), &recent, 0, 0);
        assert_eq!(bd.history_count, 0, "no messages should give count 0");
        assert_eq!(bd.history_tokens, 0, "no messages should give 0 tokens");
        assert_eq!(bd.identity_tokens, 0, "empty identity should give 0 tokens");
        assert_eq!(
            bd.memory_pipeline_tokens, 0,
            "no memory should give 0 tokens"
        );
    }

    #[test]
    fn context_breakdown_with_identity_has_nonzero_identity_tokens() {
        let identity = IdentityFiles {
            soul: Some("I am a helpful assistant.".to_string()),
            ..IdentityFiles::default()
        };
        let memory = no_memory();
        let recent = RecentMessages::new();

        let bd =
            compute_context_breakdown(&identity, &memory, &PromptContext::default(), &recent, 0, 0);
        assert!(
            bd.identity_tokens > 0,
            "non-empty identity should give positive identity token count"
        );
    }

    #[test]
    fn context_breakdown_history_count_matches_messages() {
        let identity = IdentityFiles::default();
        let memory = no_memory();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("hello"));
        recent.push(Message::assistant("hi there", None));

        let bd =
            compute_context_breakdown(&identity, &memory, &PromptContext::default(), &recent, 0, 0);
        assert_eq!(
            bd.history_count, 2,
            "history count should match message count"
        );
        assert!(
            bd.history_tokens > 0,
            "non-empty history should have positive token count"
        );
    }

    #[test]
    fn context_breakdown_memory_counts_separately() {
        let identity = IdentityFiles::default();
        let observations = "Episode 1: the user asked about rust.".to_string();
        let memory = MemoryContext {
            observations: Some(&observations),
            recent_context: None,
        };
        let recent = RecentMessages::new();

        let bd =
            compute_context_breakdown(&identity, &memory, &PromptContext::default(), &recent, 0, 0);
        assert!(
            bd.memory_pipeline_tokens > 0,
            "observation content should produce nonzero memory_pipeline_tokens"
        );
    }

    #[test]
    fn context_breakdown_tool_tokens_passed_through() {
        let identity = IdentityFiles::default();
        let memory = no_memory();
        let recent = RecentMessages::new();

        let bd = compute_context_breakdown(
            &identity,
            &memory,
            &PromptContext::default(),
            &recent,
            42,
            7,
        );
        assert_eq!(
            bd.system_tool_tokens, 42,
            "system_tool_tokens should pass through unchanged"
        );
        assert_eq!(
            bd.mcp_tool_tokens, 7,
            "mcp_tool_tokens should pass through unchanged"
        );
    }

    #[test]
    fn context_breakdown_prompt_context_tokens() {
        let identity = IdentityFiles::default();
        let memory = no_memory();
        let recent = RecentMessages::new();

        let prompt_ctx = PromptContext {
            skills: SkillsContext {
                index: Some("<available_skills/>"),
                active_instructions: Some("<active_skill>instructions</active_skill>"),
            },
        };

        let bd = compute_context_breakdown(&identity, &memory, &prompt_ctx, &recent, 0, 0);
        assert!(
            bd.skills_index_tokens > 0,
            "skills index should produce nonzero tokens"
        );
        assert!(
            bd.active_skills_tokens > 0,
            "active skills should produce nonzero tokens"
        );
    }
}
