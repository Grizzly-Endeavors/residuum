//! System prompt content builders.

use crate::time::{format_display_datetime, format_relative_time};
use crate::workspace::identity::IdentityFiles;

use super::types::{MemoryContext, ProjectsContext, SkillsContext, StatusLine, SubagentsContext};

/// Build the `[Current Time: ...][Last Message: ...][Message Source: ...][Unread Inbox: N]` tag string.
pub(super) fn build_status_line(ctx: &StatusLine) -> String {
    use std::fmt::Write as _;

    let current = format_display_datetime(ctx.now);
    let mut tag = format!("[Current Time: {current}]");

    if let Some(prev) = ctx.last_message_at {
        let delta = ctx.now - prev;
        let relative = format_relative_time(delta);
        _ = write!(tag, "[Last Message: {relative}]");
    }

    if let Some(source) = &ctx.message_source {
        _ = write!(tag, "[Message Source: {source}]");
    }

    if ctx.unread_inbox_count > 0 {
        _ = write!(tag, "[Unread Inbox: {}]", ctx.unread_inbox_count);
    }

    tag
}

/// Build a minimal system prompt for background sub-agent turns.
///
/// Includes optional preset instructions, ENVIRONMENT.md, USER.md, projects index,
/// active project context, skills index, and active skill instructions.
///
/// Excludes SOUL, AGENTS, MEMORY, observations, recent context, and the subagents
/// index (subagents cannot spawn other subagents).
///
/// Assembly order (matching main prompt structure for cache efficiency):
/// 1. `AGENT_INSTRUCTIONS` (preset instructions, if any)
/// 2. `ENVIRONMENT.md`
/// 3. `USER.md`
/// 4. `PROJECTS_INDEX`
/// 5. `SKILLS_INDEX`
/// 6. `ACTIVE_PROJECT` (when a project is active)
/// 7. `ACTIVE_SKILLS` (when skills are loaded)
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

    if let Some(active) = projects_ctx.active_context
        && !active.is_empty()
    {
        parts.push(format!("<ACTIVE_PROJECT>\n{active}\n</ACTIVE_PROJECT>"));
    }

    if let Some(active) = skills_ctx.active_instructions
        && !active.is_empty()
    {
        parts.push(format!("<ACTIVE_SKILLS>\n{active}\n</ACTIVE_SKILLS>"));
    }

    parts.join("\n\n")
}

/// Build the system prompt content from identity files.
///
/// Assembly order (designed to maximize prompt caching efficiency):
/// 1. `SOUL.md`
/// 2. `AGENTS.md`
/// 3. `BOOTSTRAP.md` (first-run only, deleted after first conversation)
/// 4. `ENVIRONMENT.md`
/// 5. `USER.md`
/// 6. `MEMORY.md`
/// 7. `OBSERVATION_LOG` (if present)
/// 8. `RECENT_CONTEXT` (if present)
/// 9. `SUBAGENTS_INDEX` (available presets listing)
/// 10. `PROJECTS_INDEX` (always present after bootstrap)
/// 11. `SKILLS_INDEX` (available skills listing)
/// 12. `ACTIVE_PROJECT` (when a project is active)
/// 13. `ACTIVE_SKILLS` (when skills are loaded)
///
/// Static sections (1-5) form a stable cache prefix shared across all conversations.
/// Dynamic sections (6-8) update as memory changes. Indices (9-11) appear before
/// active sections (12-13) to maximize cache reuse as projects/skills change.
pub(super) fn build_system_content(
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

    if let Some(bootstrap) = &identity.bootstrap {
        parts.push(format!("<BOOTSTRAP.md>\n{bootstrap}\n</BOOTSTRAP.md>"));
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

    if let Some(idx) = subagents_ctx.index
        && !idx.is_empty()
    {
        parts.push(format!("<SUBAGENTS_INDEX>\n{idx}\n</SUBAGENTS_INDEX>"));
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

    if let Some(active) = projects_ctx.active_context
        && !active.is_empty()
    {
        parts.push(format!("<ACTIVE_PROJECT>\n{active}\n</ACTIVE_PROJECT>"));
    }

    if let Some(active) = skills_ctx.active_instructions
        && !active.is_empty()
    {
        parts.push(format!("<ACTIVE_SKILLS>\n{active}\n</ACTIVE_SKILLS>"));
    }

    parts.join("\n\n")
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use chrono::NaiveDateTime;

    use super::*;

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
    fn system_content_includes_identity() {
        let identity = IdentityFiles {
            soul: Some("I am a test agent".to_string()),
            agents: Some("Agents content".to_string()),
            user: Some("User likes Rust".to_string()),
            ..IdentityFiles::default()
        };

        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
        );
        assert!(
            content.contains("test agent"),
            "should include soul content"
        );
        assert!(
            content.contains("User likes Rust"),
            "should include user content"
        );
        assert!(
            content.contains("Agents content"),
            "should include agents content"
        );
        assert!(
            content.contains("<AGENTS.md>"),
            "should wrap agents content in AGENTS.md tags"
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
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
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
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
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
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
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
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
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
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
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
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
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
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
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

    // ── Bootstrap context tests ───────────────────────────────────────────────

    #[test]
    fn bootstrap_injected_between_agents_and_environment() {
        let identity = IdentityFiles {
            agents: Some("agent rules".to_string()),
            bootstrap: Some("first run guidance".to_string()),
            environment: Some("env notes".to_string()),
            ..IdentityFiles::default()
        };
        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
        );

        assert!(
            content.contains("<BOOTSTRAP.md>\nfirst run guidance\n</BOOTSTRAP.md>"),
            "bootstrap should be wrapped in BOOTSTRAP.md tags"
        );

        let agents_close = content.find("</AGENTS.md>").unwrap();
        let bootstrap_open = content.find("<BOOTSTRAP.md>").unwrap();
        let env_open = content.find("<ENVIRONMENT.md>").unwrap();
        assert!(
            agents_close < bootstrap_open,
            "AGENTS.md should close before BOOTSTRAP.md opens"
        );
        assert!(
            bootstrap_open < env_open,
            "BOOTSTRAP.md should open before ENVIRONMENT.md"
        );
    }

    #[test]
    fn bootstrap_none_skipped() {
        let identity = IdentityFiles {
            agents: Some("agent rules".to_string()),
            ..IdentityFiles::default()
        };
        let content = build_system_content(
            &identity,
            &no_memory(),
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
        );

        assert!(
            !content.contains("BOOTSTRAP.md"),
            "None bootstrap should not appear in prompt"
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
            &ProjectsContext::default(),
            &skills,
            &SubagentsContext::default(),
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
            &ProjectsContext::default(),
            &skills,
            &SubagentsContext::default(),
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
    fn section_order_subagents_projects_skills_active() {
        let identity = IdentityFiles::default();
        let projects = ProjectsContext {
            index: Some("| Name | Status |"),
            active_context: Some("**Project:** test"),
        };
        let skills = SkillsContext {
            index: Some("<available_skills/>"),
            active_instructions: Some("<active_skill/>"),
        };
        let subagents = SubagentsContext {
            index: Some("<presets/>"),
        };
        let content = build_system_content(&identity, &no_memory(), &projects, &skills, &subagents);

        // Verify the order: SUBAGENTS_INDEX → PROJECTS_INDEX → SKILLS_INDEX → ACTIVE_PROJECT → ACTIVE_SKILLS
        let subagents_open = content.find("<SUBAGENTS_INDEX>");
        let projects_open = content.find("<PROJECTS_INDEX>");
        let skills_open = content.find("<SKILLS_INDEX>");
        let active_proj_open = content.find("<ACTIVE_PROJECT>");
        let active_skills_open = content.find("<ACTIVE_SKILLS>");

        assert!(
            subagents_open.is_some()
                && projects_open.is_some()
                && skills_open.is_some()
                && active_proj_open.is_some()
                && active_skills_open.is_some(),
            "all sections should exist"
        );

        let sub = subagents_open.unwrap();
        let proj = projects_open.unwrap();
        let skl = skills_open.unwrap();
        let act_proj = active_proj_open.unwrap();
        let act_skl = active_skills_open.unwrap();

        assert!(
            sub < proj,
            "SUBAGENTS_INDEX should come before PROJECTS_INDEX"
        );
        assert!(proj < skl, "PROJECTS_INDEX should come before SKILLS_INDEX");
        assert!(
            skl < act_proj,
            "SKILLS_INDEX should come before ACTIVE_PROJECT"
        );
        assert!(
            act_proj < act_skl,
            "ACTIVE_PROJECT should come before ACTIVE_SKILLS"
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
            &ProjectsContext::default(),
            &skills,
            &SubagentsContext::default(),
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
            &ProjectsContext::default(),
            &SkillsContext::default(),
            &SubagentsContext::default(),
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
        assert!(!content.contains("AGENTS.md"), "should exclude AGENTS.md");
        assert!(!content.contains("MEMORY.md"), "should exclude MEMORY.md");
        assert!(
            !content.contains("OBSERVATION_LOG"),
            "should exclude OBSERVATION_LOG"
        );
        assert!(
            !content.contains("RECENT_CONTEXT"),
            "should exclude RECENT_CONTEXT"
        );
        assert!(
            !content.contains("SUBAGENTS_INDEX"),
            "should exclude SUBAGENTS_INDEX"
        );
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
    fn subagent_system_content_includes_active_skills() {
        let identity = IdentityFiles::default();
        let skills_ctx = SkillsContext {
            index: Some("<available_skills/>"),
            active_instructions: Some("<active_skill>instructions</active_skill>"),
        };
        let content = build_subagent_system_content(
            &identity,
            &ProjectsContext::default(),
            &skills_ctx,
            None,
        );
        assert!(
            content.contains("<ACTIVE_SKILLS>"),
            "active skills section should appear in subagent system prompt"
        );
        assert!(
            content.contains("instructions"),
            "active skill instructions should appear in subagent system prompt"
        );
    }

    #[test]
    fn subagent_system_content_skills_index_empty_skipped() {
        let identity = IdentityFiles::default();
        let skills_ctx = SkillsContext {
            index: Some(""),
            active_instructions: None,
        };
        let content = build_subagent_system_content(
            &identity,
            &ProjectsContext::default(),
            &skills_ctx,
            None,
        );
        assert!(
            !content.contains("SKILLS_INDEX"),
            "empty skills index should be skipped"
        );
    }

    #[test]
    fn subagent_system_content_includes_active_project() {
        let identity = IdentityFiles::default();
        let projects_ctx = ProjectsContext {
            index: Some("| proj | status |"),
            active_context: Some("**Current Project:** test-proj"),
        };
        let content = build_subagent_system_content(
            &identity,
            &projects_ctx,
            &SkillsContext::default(),
            None,
        );
        assert!(
            content.contains("<ACTIVE_PROJECT>"),
            "active project section should appear in subagent system prompt"
        );
        assert!(
            content.contains("test-proj"),
            "active project context should appear in subagent system prompt"
        );
    }

    #[test]
    fn subagent_system_content_section_order() {
        let identity = IdentityFiles {
            environment: Some("env content".to_string()),
            user: Some("user content".to_string()),
            ..IdentityFiles::default()
        };
        let projects_ctx = ProjectsContext {
            index: Some("projects"),
            active_context: Some("active proj"),
        };
        let skills_ctx = SkillsContext {
            index: Some("skills"),
            active_instructions: Some("active skills"),
        };
        let content = build_subagent_system_content(&identity, &projects_ctx, &skills_ctx, None);

        // Verify order: ENVIRONMENT → USER → PROJECTS_INDEX → SKILLS_INDEX → ACTIVE_PROJECT → ACTIVE_SKILLS
        let env_pos = content.find("env content").unwrap();
        let user_pos = content.find("user content").unwrap();
        let proj_idx_pos = content.find("<PROJECTS_INDEX>").unwrap();
        let skl_idx_pos = content.find("<SKILLS_INDEX>").unwrap();
        let active_proj_pos = content.find("<ACTIVE_PROJECT>").unwrap();
        let active_skl_pos = content.find("<ACTIVE_SKILLS>").unwrap();

        assert!(
            env_pos < user_pos
                && user_pos < proj_idx_pos
                && proj_idx_pos < skl_idx_pos
                && skl_idx_pos < active_proj_pos
                && active_proj_pos < active_skl_pos,
            "sections should appear in order: ENVIRONMENT, USER, PROJECTS_INDEX, SKILLS_INDEX, ACTIVE_PROJECT, ACTIVE_SKILLS"
        );
    }

    #[test]
    fn subagent_system_content_includes_preset_instructions() {
        let identity = IdentityFiles {
            environment: Some("env notes".to_string()),
            ..IdentityFiles::default()
        };
        let content = build_subagent_system_content(
            &identity,
            &ProjectsContext::default(),
            &SkillsContext::default(),
            Some("Do this specific task"),
        );

        assert!(
            content.contains("<AGENT_INSTRUCTIONS>"),
            "should wrap preset instructions in AGENT_INSTRUCTIONS tags"
        );
        assert!(
            content.contains("Do this specific task"),
            "should include preset instruction content"
        );
        // AGENT_INSTRUCTIONS must appear before ENVIRONMENT.md
        let instructions_pos = content.find("<AGENT_INSTRUCTIONS>").unwrap();
        let env_pos = content.find("<ENVIRONMENT.md>").unwrap();
        assert!(
            instructions_pos < env_pos,
            "AGENT_INSTRUCTIONS should come before ENVIRONMENT.md"
        );
    }

    // ── build_status_line tests ───────────────────────────────────────────────

    #[test]
    fn status_line_no_last_message() {
        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: None,
            unread_inbox_count: 0,
        };
        let result = build_status_line(&ctx);
        assert!(
            result.contains("[Current Time:"),
            "should have current time tag"
        );
        assert!(
            !result.contains("[Last Message:"),
            "should not have last message tag"
        );
    }

    #[test]
    fn status_line_with_last_message() {
        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: Some(dt(2026, 2, 22, 16, 45)),
            message_source: None,
            unread_inbox_count: 0,
        };
        let result = build_status_line(&ctx);
        assert!(
            result.contains("[Current Time:"),
            "should have current time tag"
        );
        assert!(
            result.contains("[Last Message:"),
            "should have last message tag"
        );
    }

    #[test]
    fn status_line_with_source() {
        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: Some("discord".to_string()),
            unread_inbox_count: 0,
        };
        let result = build_status_line(&ctx);
        assert!(
            result.contains("[Message Source: discord]"),
            "should have message source tag"
        );
    }

    #[test]
    fn status_line_with_nonzero_unread() {
        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: None,
            unread_inbox_count: 5,
        };
        let result = build_status_line(&ctx);
        assert!(
            result.contains("[Unread Inbox: 5]"),
            "should have unread inbox tag with count"
        );
    }

    #[test]
    fn status_line_zero_unread_omitted() {
        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: None,
            unread_inbox_count: 0,
        };
        let result = build_status_line(&ctx);
        assert!(
            !result.contains("[Unread Inbox:"),
            "should not have unread inbox tag when count is 0"
        );
    }
}
