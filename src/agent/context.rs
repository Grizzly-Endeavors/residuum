//! Context assembly: builds the system prompt from identity files.

use chrono::NaiveDateTime;

use crate::models::Message;
use crate::time::{format_display_datetime, format_relative_time};
use crate::workspace::identity::IdentityFiles;

use super::recent_messages::RecentMessages;

/// Ephemeral context injected before the last user message in each LLM call.
pub struct StatusLine {
    /// Current local time.
    pub now: NaiveDateTime,
    /// When the previous user message was sent (if any).
    pub last_message_at: Option<NaiveDateTime>,
    /// Which channel this message arrived from (e.g. `"websocket"`, `"discord"`).
    pub message_source: Option<String>,
    /// Number of unread inbox items (0 → tag omitted).
    pub unread_inbox_count: usize,
}

/// Memory-related context injected into the system prompt.
///
/// Groups observation log and recent narrative context to avoid parameter
/// explosion on `assemble_system_prompt` and `execute_turn`.
pub struct MemoryContext<'a> {
    /// Formatted observation log content (if present).
    pub observations: Option<&'a str>,
    /// Narrative summary from the most recent observation (if present).
    pub recent_context: Option<&'a str>,
}

/// Projects-related context injected into the system prompt.
pub struct ProjectsContext<'a> {
    /// Formatted project index (always present after bootstrap).
    pub index: Option<&'a str>,
    /// Formatted active project context (only when a project is active).
    pub active_context: Option<&'a str>,
}

impl ProjectsContext<'_> {
    /// Empty projects context (no index, no active project).
    #[must_use]
    pub fn none() -> ProjectsContext<'static> {
        ProjectsContext {
            index: None,
            active_context: None,
        }
    }
}

/// Skills-related context injected into the system prompt.
pub struct SkillsContext<'a> {
    /// Formatted skills index XML (available skills listing).
    pub index: Option<&'a str>,
    /// Formatted active skill instructions XML.
    pub active_instructions: Option<&'a str>,
}

impl SkillsContext<'_> {
    /// Empty skills context (no index, no active skills).
    #[must_use]
    pub fn none() -> SkillsContext<'static> {
        SkillsContext {
            index: None,
            active_instructions: None,
        }
    }
}

/// Subagent-preset-related context injected into the system prompt.
pub struct SubagentsContext<'a> {
    /// Formatted subagent presets index XML (available presets listing).
    pub index: Option<&'a str>,
}

impl SubagentsContext<'_> {
    /// Empty subagents context (no index).
    #[must_use]
    pub fn none() -> SubagentsContext<'static> {
        SubagentsContext { index: None }
    }
}

/// Bundle of external context injected into the system prompt.
///
/// Groups projects, skills, and subagents context into a single struct to
/// reduce argument count on functions that thread all three through.
pub struct PromptContext<'a> {
    pub projects: ProjectsContext<'a>,
    pub skills: SkillsContext<'a>,
    pub subagents: SubagentsContext<'a>,
}

impl PromptContext<'_> {
    /// Empty prompt context (no projects, skills, or subagents).
    #[must_use]
    pub fn none() -> PromptContext<'static> {
        PromptContext {
            projects: ProjectsContext::none(),
            skills: SkillsContext::none(),
            subagents: SubagentsContext::none(),
        }
    }
}

/// Assemble the full message list for a model call.
///
/// Creates a system message from identity files and observation log content,
/// then appends the recent messages. When a `StatusLine` is provided, a
/// system message with the current time (and optionally how long since the
/// last message) is inserted immediately before the last user message.
#[must_use]
pub(super) fn assemble_system_prompt(
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

/// Build the `[Current Time: ...][Last Message: ...][Message Source: ...][Unread Inbox: N]` tag string.
fn build_status_line(ctx: &StatusLine) -> String {
    let current = format_display_datetime(ctx.now);
    let mut tag = match ctx.last_message_at {
        Some(prev) => {
            let delta = ctx.now - prev;
            let relative = format_relative_time(delta);
            format!("[Current Time: {current}][Last Message: {relative}]")
        }
        None => format!("[Current Time: {current}]"),
    };

    if let Some(source) = &ctx.message_source {
        tag = format!("{tag}[Message Source: {source}]");
    }

    if ctx.unread_inbox_count > 0 {
        tag = format!("{tag}[Unread Inbox: {}]", ctx.unread_inbox_count);
    }

    tag
}

/// Build a minimal system prompt for background sub-agent turns.
///
/// Includes optional preset instructions, ENVIRONMENT.md, USER.md, the projects
/// index, and skills index. Excludes SOUL, AGENTS, MEMORY,
/// observations, recent context, active skill instructions, and the
/// subagents index (subagents cannot spawn other subagents).
#[must_use]
pub(crate) fn build_subagent_system_content(
    identity: &IdentityFiles,
    projects_ctx: &ProjectsContext<'_>,
    skills_ctx: &SkillsContext<'_>,
    preset_instructions: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    if let Some(instructions) = preset_instructions
        && !instructions.is_empty()
    {
        parts.push(format!(
            "<AGENT_INSTRUCTIONS>\n{instructions}\n</AGENT_INSTRUCTIONS>"
        ));
    }

    if let Some(environment_md) = &identity.environment {
        parts.push(format!(
            "<ENVIRONMENT.md>\n{environment_md}\n</ENVIRONMENT.md>"
        ));
    }

    if let Some(user) = &identity.user {
        parts.push(format!("<USER.md>\n{user}\n</USER.md>"));
    }

    if let Some(idx) = projects_ctx.index
        && !idx.is_empty()
    {
        parts.push(format!("<PROJECTS_INDEX>\n{idx}\n</PROJECTS_INDEX>"));
    }

    if let Some(idx) = skills_ctx.index
        && !idx.is_empty()
    {
        parts.push(format!("<SKILLS_INDEX>\n{idx}\n</SKILLS_INDEX>"));
    }

    parts.join("\n\n")
}

/// Build the system prompt content from identity files.
///
/// Assembly order:
/// 1. SOUL.md content
/// 2. AGENTS.md content
/// 3. ENVIRONMENT.md content
/// 4. USER.md content
/// 5. MEMORY.md content
/// 6. Observation log (if present)
/// 7. Recent context / narrative summary (if present)
/// 8. Projects index (always present after bootstrap)
/// 9. Active project context (when a project is active)
/// 10. Skills index (available skills listing)
/// 11. Active skill instructions (when skills are loaded)
fn build_system_content(
    identity: &IdentityFiles,
    memory_ctx: &MemoryContext<'_>,
    projects_ctx: &ProjectsContext<'_>,
    skills_ctx: &SkillsContext<'_>,
    subagents_ctx: &SubagentsContext<'_>,
) -> String {
    let mut parts = Vec::new();

    if let Some(soul) = &identity.soul {
        parts.push(format!("<SOUL.md>\n{soul}\n</SOUL.md>"));
    }

    if let Some(agents) = &identity.agents {
        parts.push(format!("<AGENTS.md>\n{agents}\n</AGENTS.md>"));
    }

    if let Some(environment_md) = &identity.environment {
        parts.push(format!(
            "<ENVIRONMENT.md>\n{environment_md}\n</ENVIRONMENT.md>"
        ));
    }

    if let Some(user) = &identity.user {
        parts.push(format!("<USER.md>\n{user}\n</USER.md>"));
    }

    if let Some(memory) = &identity.memory {
        parts.push(format!("<MEMORY.md>\n{memory}\n</MEMORY.md>"));
    }

    if let Some(obs) = memory_ctx.observations
        && !obs.is_empty()
    {
        parts.push(format!("<OBSERVATION_LOG>\n{obs}\n</OBSERVATION_LOG>"));
    }

    if let Some(ctx) = memory_ctx.recent_context
        && !ctx.is_empty()
    {
        parts.push(format!("<RECENT_CONTEXT>\n{ctx}\n</RECENT_CONTEXT>"));
    }

    if let Some(idx) = projects_ctx.index
        && !idx.is_empty()
    {
        parts.push(format!("<PROJECTS_INDEX>\n{idx}\n</PROJECTS_INDEX>"));
    }

    if let Some(active) = projects_ctx.active_context
        && !active.is_empty()
    {
        parts.push(format!("<ACTIVE_PROJECT>\n{active}\n</ACTIVE_PROJECT>"));
    }

    if let Some(idx) = skills_ctx.index
        && !idx.is_empty()
    {
        parts.push(format!("<SKILLS_INDEX>\n{idx}\n</SKILLS_INDEX>"));
    }

    if let Some(active) = skills_ctx.active_instructions
        && !active.is_empty()
    {
        parts.push(format!("<ACTIVE_SKILLS>\n{active}\n</ACTIVE_SKILLS>"));
    }

    if let Some(idx) = subagents_ctx.index
        && !idx.is_empty()
    {
        parts.push(format!("<SUBAGENTS_INDEX>\n{idx}\n</SUBAGENTS_INDEX>"));
    }

    parts.join("\n\n")
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test code uses unwrap and indexing for clarity"
)]
mod tests {
    use super::*;
    use crate::models::Role;

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
    fn system_content_includes_identity() {
        let identity = IdentityFiles {
            soul: Some("I am a test agent".to_string()),
            user: Some("User likes Rust".to_string()),
            ..IdentityFiles::default()
        };

        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );
        assert!(
            content.contains("test agent"),
            "should include soul content"
        );
        assert!(
            content.contains("User likes Rust"),
            "should include user content"
        );
    }

    #[test]
    fn system_content_includes_observations() {
        let identity = IdentityFiles::default();

        let mem = MemoryContext {
            observations: Some("episode ep-001: user prefers concise output"),
            recent_context: None,
        };
        let content = build_system_content(
            &identity,
            &mem,
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );

        assert!(
            content.contains("<OBSERVATION_LOG>"),
            "should have observation log tag"
        );
        assert!(
            content.contains("</OBSERVATION_LOG>"),
            "should have closing observation log tag"
        );
        assert!(
            content.contains("user prefers concise output"),
            "should include observation content"
        );
    }

    #[test]
    fn system_content_skips_empty_observations() {
        let identity = IdentityFiles::default();

        let mem = MemoryContext {
            observations: Some(""),
            recent_context: None,
        };
        let content = build_system_content(
            &identity,
            &mem,
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );
        assert!(
            !content.contains("OBSERVATION_LOG"),
            "empty observations should be skipped"
        );
    }

    #[test]
    fn system_content_skips_none_observations() {
        let identity = IdentityFiles::default();

        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );
        assert!(
            !content.contains("OBSERVATION_LOG"),
            "None observations should be skipped"
        );
    }

    #[test]
    fn sections_wrapped_in_xml_tags() {
        let identity = IdentityFiles {
            soul: Some("I am the soul.".to_string()),
            memory: Some("User prefers Rust.".to_string()),
            ..IdentityFiles::default()
        };
        let mem = MemoryContext {
            observations: Some("some observation"),
            recent_context: None,
        };
        let content = build_system_content(
            &identity,
            &mem,
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );

        assert!(
            content.contains("<SOUL.md>\nI am the soul.\n</SOUL.md>"),
            "soul should be wrapped in SOUL.md tags"
        );
        assert!(
            content.contains("<MEMORY.md>\nUser prefers Rust.\n</MEMORY.md>"),
            "memory should be wrapped in MEMORY.md tags"
        );
        assert!(
            content.contains("<OBSERVATION_LOG>\nsome observation\n</OBSERVATION_LOG>"),
            "observations should be wrapped in OBSERVATION_LOG tags"
        );

        // Memory and observation log should be clearly separate sections
        let memory_close = content.find("</MEMORY.md>");
        let obs_open = content.find("<OBSERVATION_LOG>");
        assert!(
            memory_close.is_some() && obs_open.is_some(),
            "both sections should exist"
        );
        assert!(
            memory_close < obs_open,
            "memory should close before observation log opens"
        );
    }

    #[test]
    fn system_content_includes_recent_context() {
        let identity = IdentityFiles::default();
        let mem = MemoryContext {
            observations: None,
            recent_context: Some("We were implementing a caching layer."),
        };
        let content = build_system_content(
            &identity,
            &mem,
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );

        assert!(
            content.contains("<RECENT_CONTEXT>"),
            "should have recent context tag"
        );
        assert!(
            content.contains("implementing a caching layer"),
            "should include recent context content"
        );
    }

    #[test]
    fn system_content_skips_empty_recent_context() {
        let identity = IdentityFiles::default();
        let mem = MemoryContext {
            observations: None,
            recent_context: Some(""),
        };
        let content = build_system_content(
            &identity,
            &mem,
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );
        assert!(
            !content.contains("RECENT_CONTEXT"),
            "empty recent context should be skipped"
        );
    }

    #[test]
    fn recent_context_after_observation_log() {
        let identity = IdentityFiles::default();
        let mem = MemoryContext {
            observations: Some("some observations"),
            recent_context: Some("narrative summary"),
        };
        let content = build_system_content(
            &identity,
            &mem,
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );

        let obs_close = content.find("</OBSERVATION_LOG>");
        let ctx_open = content.find("<RECENT_CONTEXT>");
        assert!(
            obs_close.is_some() && ctx_open.is_some(),
            "both sections should exist"
        );
        assert!(
            obs_close < ctx_open,
            "observation log should close before recent context opens"
        );
    }

    fn dt(year: i32, month: u32, day: u32, hour: u32, min: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, min, 0)
            .unwrap()
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

    // ── Skills context tests ─────────────────────────────────────────────────

    #[test]
    fn skills_index_in_prompt() {
        let identity = IdentityFiles::default();
        let skills = SkillsContext {
            index: Some(
                "<available_skills>\n  <skill><name>pdf</name></skill>\n</available_skills>",
            ),
            active_instructions: None,
        };
        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::none(),
            &skills,
            &SubagentsContext::none(),
        );
        assert!(
            content.contains("<SKILLS_INDEX>"),
            "should have skills index section"
        );
        assert!(
            content.contains("</SKILLS_INDEX>"),
            "should have closing skills index tag"
        );
        assert!(
            content.contains("<name>pdf</name>"),
            "should contain skill name"
        );
    }

    #[test]
    fn active_skills_in_prompt() {
        let identity = IdentityFiles::default();
        let skills = SkillsContext {
            index: None,
            active_instructions: Some(
                "<active_skill name=\"pdf\">\nUse this for PDFs.\n</active_skill>",
            ),
        };
        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::none(),
            &skills,
            &SubagentsContext::none(),
        );
        assert!(
            content.contains("<ACTIVE_SKILLS>"),
            "should have active skills section"
        );
        assert!(
            content.contains("Use this for PDFs"),
            "should contain skill body"
        );
    }

    #[test]
    fn skills_after_projects() {
        let identity = IdentityFiles::default();
        let projects = ProjectsContext {
            index: Some("| Name | Status |"),
            active_context: Some("**Project:** test"),
        };
        let skills = SkillsContext {
            index: Some("<available_skills/>"),
            active_instructions: Some("<active_skill/>"),
        };
        let content = build_system_content(
            &identity,
            &no_memory(),
            &projects,
            &skills,
            &SubagentsContext::none(),
        );

        let project_close = content.find("</ACTIVE_PROJECT>");
        let skills_open = content.find("<SKILLS_INDEX>");
        assert!(
            project_close.is_some() && skills_open.is_some(),
            "both sections should exist"
        );
        assert!(
            project_close < skills_open,
            "skills should appear after projects"
        );
    }

    #[test]
    fn skills_empty_index_skipped() {
        let identity = IdentityFiles::default();
        let skills = SkillsContext {
            index: Some(""),
            active_instructions: None,
        };
        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::none(),
            &skills,
            &SubagentsContext::none(),
        );
        assert!(
            !content.contains("SKILLS_INDEX"),
            "empty skills index should be skipped"
        );
    }

    #[test]
    fn skills_none_skipped() {
        let identity = IdentityFiles::default();
        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::none(),
            &SkillsContext::none(),
            &SubagentsContext::none(),
        );
        assert!(
            !content.contains("SKILLS_INDEX"),
            "None skills index should be skipped"
        );
        assert!(
            !content.contains("ACTIVE_SKILLS"),
            "None active skills should be skipped"
        );
    }

    // ── build_subagent_system_content tests ──────────────────────────────────

    #[test]
    fn subagent_system_content_includes_environment_user_projects_skills() {
        let identity = IdentityFiles {
            soul: Some("SOUL content".to_string()),
            environment: Some("env notes".to_string()),
            user: Some("user prefs".to_string()),
            ..IdentityFiles::default()
        };
        let projects_ctx = ProjectsContext {
            index: Some("| proj | active |"),
            active_context: None,
        };
        let skills_ctx = SkillsContext {
            index: Some("<available_skills/>"),
            active_instructions: None,
        };
        let content = build_subagent_system_content(&identity, &projects_ctx, &skills_ctx, None);

        assert!(!content.contains("SOUL"), "should exclude SOUL.md");
        assert!(
            content.contains("env notes"),
            "should include ENVIRONMENT.md"
        );
        assert!(content.contains("user prefs"), "should include USER.md");
        assert!(
            content.contains("| proj | active |"),
            "should include projects index"
        );
        assert!(
            content.contains("<SKILLS_INDEX>"),
            "should include skills index"
        );
    }

    #[test]
    fn subagent_system_content_excludes_active_skills() {
        // Active skill instructions are not included in the sub-agent system prompt
        let identity = IdentityFiles::default();
        let skills_ctx = SkillsContext {
            index: Some("<available_skills/>"),
            active_instructions: Some("<active_skill>instructions</active_skill>"),
        };
        let content =
            build_subagent_system_content(&identity, &ProjectsContext::none(), &skills_ctx, None);
        assert!(
            !content.contains("instructions"),
            "active skill instructions should not appear in subagent system prompt"
        );
    }

    #[test]
    fn subagent_system_content_skills_index_empty_skipped() {
        let identity = IdentityFiles::default();
        let skills_ctx = SkillsContext {
            index: Some(""),
            active_instructions: None,
        };
        let content =
            build_subagent_system_content(&identity, &ProjectsContext::none(), &skills_ctx, None);
        assert!(
            !content.contains("SKILLS_INDEX"),
            "empty skills index should be skipped"
        );
    }
}
