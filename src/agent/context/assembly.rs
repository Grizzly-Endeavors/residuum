//! System prompt assembly: combines identity, memory, and context into messages.

use crate::memory::tokens::{estimate_message_tokens, estimate_tokens};
use crate::models::Message;
use crate::workspace::identity::IdentityFiles;

use super::super::recent_messages::RecentMessages;
use super::prompt::{build_status_line, build_system_content};
use super::types::{ContextBreakdown, MemoryContext, PromptContext, StatusLine};

/// Compute a per-section token breakdown for the current agent context.
pub(in crate::agent) fn compute_context_breakdown(
    identity: &IdentityFiles,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    recent_messages: &RecentMessages,
    tool_tokens: usize,
) -> ContextBreakdown {
    let identity_tokens = [
        identity.soul.as_deref(),
        identity.agents.as_deref(),
        identity.environment.as_deref(),
        identity.user.as_deref(),
        identity.memory.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(estimate_tokens)
    .sum();

    let observation_log_tokens = memory_ctx.observations.map_or(0, estimate_tokens)
        + memory_ctx.recent_context.map_or(0, estimate_tokens);

    let msgs = recent_messages.messages();

    ContextBreakdown {
        identity_tokens,
        observation_log_tokens,
        subagents_index_tokens: prompt_ctx.subagents.index.map_or(0, estimate_tokens),
        projects_index_tokens: prompt_ctx.projects.index.map_or(0, estimate_tokens),
        active_project_tokens: prompt_ctx
            .projects
            .active_context
            .map_or(0, estimate_tokens),
        skills_index_tokens: prompt_ctx.skills.index.map_or(0, estimate_tokens),
        active_skills_tokens: prompt_ctx
            .skills
            .active_instructions
            .map_or(0, estimate_tokens),
        tool_tokens,
        history_tokens: estimate_message_tokens(msgs),
        history_count: msgs.len(),
    }
}

/// Assemble the full message list for a model call.
///
/// Creates a system message from identity files and observation log content,
/// then appends the recent messages. When a `StatusLine` is provided, a
/// system message with the current time (and optionally how long since the
/// last message) is inserted immediately before the last user message.
#[must_use]
pub(in crate::agent) fn assemble_system_prompt(
    identity: &IdentityFiles,
    recent_messages: &RecentMessages,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    status_line: Option<&StatusLine>,
) -> Vec<Message> {
    let system_content = build_system_content(
        identity,
        memory_ctx,
        &prompt_ctx.projects,
        &prompt_ctx.skills,
        &prompt_ctx.subagents,
    );

    let conversation = recent_messages.messages();
    let mut messages = Vec::with_capacity(2 + conversation.len());

    messages.push(Message::system(system_content));
    messages.extend(conversation.iter().cloned());

    if let Some(ctx) = status_line {
        let tag = build_status_line(ctx);
        // Insert before the last user message
        if let Some(pos) = messages
            .iter()
            .rposition(|m| m.role == crate::models::Role::User)
        {
            messages.insert(pos, Message::system(tag));
        }
    }

    messages
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test code uses unwrap and indexing for clarity"
)]
mod tests {
    use chrono::NaiveDateTime;

    use super::*;
    use crate::models::Role;

    fn no_memory() -> MemoryContext<'static> {
        MemoryContext {
            observations: None,
            recent_context: None,
        }
    }

    fn dt(year: i32, month: u32, day: u32, hour: u32, min: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, min, 0)
            .unwrap()
    }

    #[test]
    fn assemble_with_empty_identity() {
        let identity = IdentityFiles::default();
        let recent = RecentMessages::new();

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            None,
        );
        assert_eq!(messages.len(), 1, "should have system message only");
        assert_eq!(
            messages.first().map(|m| &m.role),
            Some(&Role::System),
            "first message should be system"
        );
    }

    #[test]
    fn assemble_includes_message_history() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("hello"));

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            None,
        );
        assert_eq!(messages.len(), 2, "should have system + user message");
    }

    #[test]
    fn time_context_inserted_before_last_user_message() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("hello"));
        recent.push(Message::assistant("hi there", None));
        recent.push(Message::user("what time is it?"));

        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: Some(dt(2026, 2, 22, 16, 45)),
            message_source: None,
            unread_inbox_count: 0,
        };

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            Some(&ctx),
        );

        // Find the status line system message
        let time_msg = messages
            .iter()
            .find(|m| m.content.contains("[Current Time:"));
        assert!(time_msg.is_some(), "should have status line message");

        let tag = &time_msg.unwrap().content;
        assert!(
            tag.contains("Sunday Feb 22nd 2026 | 17:00"),
            "should contain formatted time, got: {tag}"
        );
        assert!(
            tag.contains("[Last Message: 15 mins ago]"),
            "should contain relative time, got: {tag}"
        );

        // Status line should be right before the last user message
        let time_pos = messages
            .iter()
            .position(|m| m.content.contains("[Current Time:"))
            .unwrap();
        assert_eq!(
            messages[time_pos + 1].content,
            "what time is it?",
            "status line should be immediately before last user message"
        );
    }

    #[test]
    fn time_context_none_no_injection() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("hello"));

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            None,
        );
        assert_eq!(messages.len(), 2, "should have system + user, no time tag");
    }

    #[test]
    fn time_context_first_message_no_last() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("first message"));

        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: None,
            unread_inbox_count: 0,
        };

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            Some(&ctx),
        );

        let time_msg = messages
            .iter()
            .find(|m| m.content.contains("[Current Time:"));
        assert!(time_msg.is_some(), "should have status line message");

        let tag = &time_msg.unwrap().content;
        assert!(
            !tag.contains("[Last Message:"),
            "should not contain last message when None, got: {tag}"
        );
    }

    #[test]
    fn time_context_includes_message_source() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("hello from discord"));

        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: Some("discord".to_string()),
            unread_inbox_count: 0,
        };

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            Some(&ctx),
        );

        let time_msg = messages
            .iter()
            .find(|m| m.content.contains("[Current Time:"));
        assert!(time_msg.is_some(), "should have time context message");

        let tag = &time_msg.as_ref().map(|m| m.content.as_str());
        assert!(
            tag.is_some_and(|t| t.contains("[Message Source: discord]")),
            "should contain message source tag, got: {tag:?}"
        );
    }

    #[test]
    fn time_context_only_before_last_user_message() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("first"));
        recent.push(Message::assistant("response", None));
        recent.push(Message::user("second"));
        recent.push(Message::assistant("response 2", None));
        recent.push(Message::user("third"));

        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: Some(dt(2026, 2, 22, 16, 0)),
            message_source: None,
            unread_inbox_count: 0,
        };

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            Some(&ctx),
        );

        // Only one time context message
        let time_count = messages
            .iter()
            .filter(|m| m.content.contains("[Current Time:"))
            .count();
        assert_eq!(
            time_count, 1,
            "should have exactly one time context message"
        );

        // It should be before "third", not "first" or "second"
        let time_pos = messages
            .iter()
            .position(|m| m.content.contains("[Current Time:"))
            .unwrap();
        assert_eq!(
            messages[time_pos + 1].content,
            "third",
            "time tag should be before the last user message"
        );
    }

    #[test]
    fn status_line_includes_unread_inbox_count() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("check inbox"));

        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: None,
            unread_inbox_count: 3,
        };

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            Some(&ctx),
        );

        let tag = messages
            .iter()
            .find(|m| m.content.contains("[Current Time:"))
            .map(|m| m.content.as_str());
        assert!(
            tag.is_some_and(|t| t.contains("[Unread Inbox: 3]")),
            "should include unread inbox count, got: {tag:?}"
        );
    }

    #[test]
    fn status_line_omits_zero_unread() {
        let identity = IdentityFiles::default();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("check inbox"));

        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: None,
            unread_inbox_count: 0,
        };

        let messages = assemble_system_prompt(
            &identity,
            &recent,
            &no_memory(),
            &PromptContext::none(),
            Some(&ctx),
        );

        let tag = messages
            .iter()
            .find(|m| m.content.contains("[Current Time:"))
            .map(|m| m.content.as_str());
        assert!(
            tag.is_some_and(|t| !t.contains("[Unread Inbox:")),
            "should not include unread inbox tag when count is 0, got: {tag:?}"
        );
    }

    // ── compute_context_breakdown tests ──────────────────────────────────────

    #[test]
    fn context_breakdown_empty_identity_no_messages() {
        let identity = IdentityFiles::default();
        let memory = no_memory();
        let recent = RecentMessages::new();

        let bd = compute_context_breakdown(&identity, &memory, &PromptContext::none(), &recent, 0);
        assert_eq!(bd.history_count, 0, "no messages should give count 0");
        assert_eq!(bd.history_tokens, 0, "no messages should give 0 tokens");
        assert_eq!(bd.identity_tokens, 0, "empty identity should give 0 tokens");
        assert_eq!(
            bd.observation_log_tokens, 0,
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

        let bd = compute_context_breakdown(&identity, &memory, &PromptContext::none(), &recent, 0);
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

        let bd = compute_context_breakdown(&identity, &memory, &PromptContext::none(), &recent, 0);
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

        let bd = compute_context_breakdown(&identity, &memory, &PromptContext::none(), &recent, 0);
        assert!(
            bd.observation_log_tokens > 0,
            "observation content should produce nonzero observation_log_tokens"
        );
    }

    #[test]
    fn context_breakdown_tool_tokens_passed_through() {
        let identity = IdentityFiles::default();
        let memory = no_memory();
        let recent = RecentMessages::new();

        let bd = compute_context_breakdown(&identity, &memory, &PromptContext::none(), &recent, 42);
        assert_eq!(
            bd.tool_tokens, 42,
            "tool_tokens should pass through unchanged"
        );
    }
}
