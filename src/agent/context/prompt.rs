//! System prompt content builders.

use crate::time::{format_display_datetime, format_relative_time};
use crate::workspace::identity::IdentityFiles;

use super::types::{MemoryContext, SkillsContext, StatusLine};

fn section(tag: &str, content: &str) -> String {
    format!("<{tag}>\n{content}\n</{tag}>")
}

/// Code-owned harness orientation, always present in the main prompt.
///
/// Unlike the other sections, this is not user-editable content from a
/// workspace file — it is static text describing what the harness provides,
/// injected unconditionally so the agent always knows its own capabilities
/// regardless of what the user has (or hasn't) written into AGENTS.md.
const HARNESS: &str = "You run on Residuum, a personal-agent harness. These systems are always available; use them without being asked:

- **Memory**: memory_search (keyword + semantic) finds past conversations, observations, and wiki pages, and memory_get retrieves an episode's transcript — search before saying you don't know or don't remember. Session runs (pulses, scheduled actions, sub-agents) merge their own findings into this same searchable memory once they complete. An automatic observer archives conversations into episodes.
- **Wiki**: your long-term knowledge lives in wiki/, one concept per Markdown page with YAML frontmatter (Open Knowledge Format). WIKI_INDEX is its root index.md; folders have their own index.md. Read the index, then read the pages you need with read_file. Record what you learn there as you learn it — facts about the user, their world, this machine, their work. Write pages as declarative facts ('User prefers X'), never as instructions to yourself ('Always do X') — imperative phrasing re-reads as a directive later. Skip anything that will be stale within days (ticket numbers, in-progress states). Activate the wiki skill before writing: it holds the page format and index rules.
- **Identity files**: SOUL.md, AGENTS.md, and USER.md are yours to edit with file tools; edits take effect next turn. USER.md holds only the user's core facts (a short, capped list); everything longer-form about the user belongs in the wiki.
- **Pulses**: HEARTBEAT.yml defines scheduled background checks (hot-reloaded, no restart needed). Three built-ins ship by default: reflection (weekly episode review, suggestions to the user inbox), memory_tending (nightly: files new knowledge from recent episodes into the wiki and USER.md), and wiki_lint (weekly wiki health check). If they are missing from HEARTBEAT.yml, offer to restore them. When the user mentions a recurring need, propose a pulse for it.
- **Inboxes**: two. Your agent inbox (inbox_list, inbox_read, inbox_archive) collects items for you to process. The user inbox (user_inbox_add) delivers items to the user's web UI — use it for background findings that should not interrupt conversation; it can carry file attachments (paths to files already on disk) alongside the title and body.
- **Scheduled actions**: one-off future tasks via the action tools; they fire once then auto-remove.
- **Sub-agents**: spawn background work with subagent_spawn, which returns its address right away. A sub-agent is an agent loop running off the main thread; pass a skill name to give it a role, and its instructions become the sub-agent's brief. Each turn's result is relayed back to you as a message, tagged with its address; use message_agent to send it a follow-up, and list_agents to see what's live. A sub-agent's result is its self-report, not verified fact — when it matters, have it return concrete handles (paths, IDs, URLs) and verify them.
- **Skills**: loadable knowledge packs in skills/*/SKILL.md, activated with skill_activate. Author new skills yourself when you keep re-deriving the same procedure.
- **Notifications**: background results are filed to the inbox; a sub-agent that ends its summary with `HEARTBEAT_URGENT` also pushes to every configured notification channel.
- **Senders**: a message from a chat interface starts with a `[From: name via interface (location)]` line naming who sent it and where. In shared spaces such as team channels, people other than the user can talk to you; that line is how you tell them apart. Messages from the web UI carry no such line and always come from the user. Keep the user's private information out of replies to other people unless the user has said otherwise.

The residuum-system skill is the authoritative reference for all of the above. Activate it before answering any question about what you can do, and whenever you are unsure whether the harness supports something.

Extend, don't just operate: authoring pulses and skills for the user's recurring needs is part of your job.";

/// Build the `[Current Time: ...][Last Message: ...][Message Source: ...]` tag string.
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

    tag
}

/// Build the system prompt content from identity files.
///
/// Assembly order (designed to maximize prompt caching efficiency):
/// 1. `SOUL.md`
/// 2. `AGENTS.md`
/// 3. `HARNESS` (code-owned, static — always present)
/// 4. `BOOTSTRAP.md` (first-run only, deleted after first conversation)
/// 5. `USER.md`
/// 6. `WIKI_INDEX` (the wiki's root `index.md`)
/// 7. `OBSERVATION_LOG` (if present)
/// 8. `RECENT_CONTEXT` (if present)
/// 9. `SKILLS_INDEX` (available skills listing)
/// 10. `ACTIVE_SKILLS` (when skills are loaded)
///
/// Static sections (1-5) form a stable cache prefix shared across all conversations.
/// Dynamic sections (6-8) update as knowledge and memory change. The skills index (9)
/// appears before the active section (10) to maximize cache reuse as skills change.
pub(super) fn build_system_content(
    identity: &IdentityFiles,
    memory_ctx: &MemoryContext<'_>,
    skills_ctx: &SkillsContext<'_>,
) -> String {
    let mut parts = Vec::new();

    if let Some(soul) = &identity.soul {
        parts.push(section("SOUL.md", soul));
    }

    if let Some(agents) = &identity.agents {
        parts.push(section("AGENTS.md", agents));
    }

    parts.push(section("HARNESS", HARNESS));

    if let Some(bootstrap) = &identity.bootstrap {
        parts.push(section("BOOTSTRAP.md", bootstrap));
    }

    if let Some(user) = &identity.user {
        parts.push(section("USER.md", user));
    }

    if let Some(wiki_index) = &identity.wiki_index {
        parts.push(section("WIKI_INDEX", wiki_index));
    }

    if let Some(obs) = memory_ctx.observations
        && !obs.is_empty()
    {
        parts.push(section("OBSERVATION_LOG", obs));
    }

    if let Some(ctx) = memory_ctx.recent_context
        && !ctx.is_empty()
    {
        parts.push(section("RECENT_CONTEXT", ctx));
    }

    if let Some(idx) = skills_ctx.index
        && !idx.is_empty()
    {
        parts.push(section("SKILLS_INDEX", idx));
    }

    if let Some(active) = skills_ctx.active_instructions
        && !active.is_empty()
    {
        parts.push(section("ACTIVE_SKILLS", active));
    }

    parts.join("\n\n")
}

#[cfg(test)]
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

        let content = build_system_content(&identity, &no_memory(), &SkillsContext::default());
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
    fn system_content_includes_harness_even_with_empty_identity() {
        let identity = IdentityFiles::default();

        let content = build_system_content(&identity, &no_memory(), &SkillsContext::default());

        assert!(
            content.contains("<HARNESS>"),
            "HARNESS section should always be present, even with empty identity files"
        );
        assert!(
            content.contains("residuum-system"),
            "HARNESS section should reference the residuum-system skill"
        );
    }

    #[test]
    fn system_content_includes_observations() {
        let identity = IdentityFiles::default();

        let mem = MemoryContext {
            observations: Some("episode ep-001: user prefers concise output"),
            recent_context: None,
        };
        let content = build_system_content(&identity, &mem, &SkillsContext::default());

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
        let content = build_system_content(&identity, &mem, &SkillsContext::default());
        assert!(
            !content.contains("OBSERVATION_LOG"),
            "empty observations should be skipped"
        );
    }

    #[test]
    fn system_content_skips_none_observations() {
        let identity = IdentityFiles::default();

        let content = build_system_content(&identity, &no_memory(), &SkillsContext::default());
        assert!(
            !content.contains("OBSERVATION_LOG"),
            "None observations should be skipped"
        );
    }

    #[test]
    fn sections_wrapped_in_xml_tags() {
        let identity = IdentityFiles {
            soul: Some("I am the soul.".to_string()),
            wiki_index: Some("- [Rust](/rust.md): user's main language".to_string()),
            ..IdentityFiles::default()
        };
        let mem = MemoryContext {
            observations: Some("some observation"),
            recent_context: None,
        };
        let content = build_system_content(&identity, &mem, &SkillsContext::default());

        assert!(
            content.contains("<SOUL.md>\nI am the soul.\n</SOUL.md>"),
            "soul should be wrapped in SOUL.md tags"
        );
        assert!(
            content
                .contains("<WIKI_INDEX>\n- [Rust](/rust.md): user's main language\n</WIKI_INDEX>"),
            "wiki index should be wrapped in WIKI_INDEX tags"
        );
        assert!(
            content.contains("<OBSERVATION_LOG>\nsome observation\n</OBSERVATION_LOG>"),
            "observations should be wrapped in OBSERVATION_LOG tags"
        );

        // Wiki index and observation log should be clearly separate sections
        let wiki_close = content.find("</WIKI_INDEX>");
        let obs_open = content.find("<OBSERVATION_LOG>");
        assert!(
            wiki_close.is_some() && obs_open.is_some(),
            "both sections should exist"
        );
        assert!(
            wiki_close < obs_open,
            "wiki index should close before observation log opens"
        );
    }

    #[test]
    fn system_content_includes_recent_context() {
        let identity = IdentityFiles::default();
        let mem = MemoryContext {
            observations: None,
            recent_context: Some("We were implementing a caching layer."),
        };
        let content = build_system_content(&identity, &mem, &SkillsContext::default());

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
        let content = build_system_content(&identity, &mem, &SkillsContext::default());
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
        let content = build_system_content(&identity, &mem, &SkillsContext::default());

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
    fn bootstrap_injected_between_agents_and_user() {
        let identity = IdentityFiles {
            agents: Some("agent rules".to_string()),
            bootstrap: Some("first run guidance".to_string()),
            user: Some("user facts".to_string()),
            ..IdentityFiles::default()
        };
        let content = build_system_content(&identity, &no_memory(), &SkillsContext::default());

        assert!(
            content.contains("<BOOTSTRAP.md>\nfirst run guidance\n</BOOTSTRAP.md>"),
            "bootstrap should be wrapped in BOOTSTRAP.md tags"
        );

        let agents_close = content.find("</AGENTS.md>").unwrap();
        let bootstrap_open = content.find("<BOOTSTRAP.md>").unwrap();
        let user_open = content.find("<USER.md>").unwrap();
        assert!(
            agents_close < bootstrap_open,
            "AGENTS.md should close before BOOTSTRAP.md opens"
        );
        assert!(
            bootstrap_open < user_open,
            "BOOTSTRAP.md should open before USER.md"
        );
    }

    #[test]
    fn bootstrap_none_skipped() {
        let identity = IdentityFiles {
            agents: Some("agent rules".to_string()),
            ..IdentityFiles::default()
        };
        let content = build_system_content(&identity, &no_memory(), &SkillsContext::default());

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
        let content = build_system_content(&identity, &no_memory(), &skills);
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
        let content = build_system_content(&identity, &no_memory(), &skills);
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
    fn skills_empty_index_skipped() {
        let identity = IdentityFiles::default();
        let skills = SkillsContext {
            index: Some(""),
            active_instructions: None,
        };
        let content = build_system_content(&identity, &no_memory(), &skills);
        assert!(
            !content.contains("SKILLS_INDEX"),
            "empty skills index should be skipped"
        );
    }

    #[test]
    fn skills_none_skipped() {
        let identity = IdentityFiles::default();
        let content = build_system_content(&identity, &no_memory(), &SkillsContext::default());
        assert!(
            !content.contains("SKILLS_INDEX"),
            "None skills index should be skipped"
        );
        assert!(
            !content.contains("ACTIVE_SKILLS"),
            "None active skills should be skipped"
        );
    }

    // ── build_status_line tests ───────────────────────────────────────────────

    #[test]
    fn status_line_no_last_message() {
        let ctx = StatusLine {
            now: dt(2026, 2, 22, 17, 0),
            last_message_at: None,
            message_source: None,
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
        };
        let result = build_status_line(&ctx);
        assert!(
            result.contains("[Message Source: discord]"),
            "should have message source tag"
        );
    }
}
