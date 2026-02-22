//! Context assembly: builds the system prompt from identity files and tool info.

use crate::models::Message;
use crate::tools::ToolRegistry;
use crate::workspace::identity::IdentityFiles;

use super::recent_messages::RecentMessages;

/// Assemble the full message list for a model call.
///
/// Creates a system message from identity files, tool listings, and
/// observation log content, then appends the recent messages.
#[must_use]
pub fn assemble_system_prompt(
    identity: &IdentityFiles,
    tools: &ToolRegistry,
    recent_messages: &RecentMessages,
    observations: Option<&str>,
) -> Vec<Message> {
    let system_content = build_system_content(identity, tools, observations);

    let mut messages = Vec::with_capacity(1 + recent_messages.messages().len());

    messages.push(Message::system(system_content));

    messages.extend(recent_messages.messages().iter().cloned());

    messages
}

/// Build the system prompt content from identity files and tool listings.
///
/// Assembly order:
/// 1. SOUL.md content
/// 2. IDENTITY.md content
/// 3. AGENTS.md content
/// 4. TOOLS.md content
/// 5. Available tool names listing
/// 6. USER.md content
/// 7. MEMORY.md content
/// 8. Observation log (if present)
fn build_system_content(
    identity: &IdentityFiles,
    tools: &ToolRegistry,
    observations: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    if let Some(soul) = &identity.soul {
        parts.push(format!("<SOUL.md>\n{soul}\n</SOUL.md>"));
    }

    if let Some(identity_md) = &identity.identity {
        parts.push(format!("<IDENTITY.md>\n{identity_md}\n</IDENTITY.md>"));
    }

    if let Some(agents) = &identity.agents {
        parts.push(format!("<AGENTS.md>\n{agents}\n</AGENTS.md>"));
    }

    if let Some(tools_md) = &identity.tools {
        parts.push(format!("<TOOLS.md>\n{tools_md}\n</TOOLS.md>"));
    }

    // List available tools
    let tool_defs = tools.definitions();
    if !tool_defs.is_empty() {
        let tool_names: Vec<&str> = tool_defs.iter().map(|t| t.name.as_str()).collect();
        parts.push(format!(
            "<AVAILABLE_TOOLS>\n{}\n</AVAILABLE_TOOLS>",
            tool_names.join(", ")
        ));
    }

    if let Some(user) = &identity.user {
        parts.push(format!("<USER.md>\n{user}\n</USER.md>"));
    }

    if let Some(memory) = &identity.memory {
        parts.push(format!("<MEMORY.md>\n{memory}\n</MEMORY.md>"));
    }

    if let Some(obs) = observations
        && !obs.is_empty()
    {
        parts.push(format!("<OBSERVATION_LOG>\n{obs}\n</OBSERVATION_LOG>"));
    }

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Role;

    #[test]
    fn assemble_with_empty_identity() {
        let identity = IdentityFiles::default();
        let tools = ToolRegistry::new();
        let recent = RecentMessages::new();

        let messages = assemble_system_prompt(&identity, &tools, &recent, None);
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
        let tools = ToolRegistry::new();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("hello"));

        let messages = assemble_system_prompt(&identity, &tools, &recent, None);
        assert_eq!(messages.len(), 2, "should have system + user message");
    }

    #[test]
    fn system_content_includes_identity() {
        let identity = IdentityFiles {
            soul: Some("I am a test agent".to_string()),
            user: Some("User likes Rust".to_string()),
            ..IdentityFiles::default()
        };
        let tools = ToolRegistry::new();

        let content = build_system_content(&identity, &tools, None);
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
    fn system_content_includes_identity_md() {
        let identity = IdentityFiles {
            soul: Some("SOUL content".to_string()),
            identity: Some("I have evolved my role over time.".to_string()),
            ..IdentityFiles::default()
        };
        let tools = ToolRegistry::new();

        let content = build_system_content(&identity, &tools, None);
        assert!(
            content.contains("SOUL content"),
            "should include soul content"
        );
        assert!(
            content.contains("evolved my role"),
            "should include identity.md content"
        );
        // IDENTITY.md should appear after SOUL.md
        let soul_pos = content.find("SOUL content").unwrap_or(usize::MAX);
        let identity_pos = content.find("evolved my role").unwrap_or(usize::MAX);
        assert!(
            soul_pos < identity_pos,
            "SOUL should appear before IDENTITY"
        );
    }

    #[test]
    fn system_content_includes_tool_listing() {
        let identity = IdentityFiles::default();
        let mut tools = ToolRegistry::new();
        tools.register_defaults(crate::tools::FileTracker::new_shared());

        let content = build_system_content(&identity, &tools, None);
        assert!(content.contains("read_file"), "should list read_file tool");
        assert!(
            content.contains("write_file"),
            "should list write_file tool"
        );
        assert!(content.contains("edit_file"), "should list edit_file tool");
        assert!(content.contains("exec"), "should list exec tool");
    }

    #[test]
    fn system_content_includes_observations() {
        let identity = IdentityFiles::default();
        let tools = ToolRegistry::new();

        let observations = "episode ep-001: user prefers concise output";
        let content = build_system_content(&identity, &tools, Some(observations));

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
        let tools = ToolRegistry::new();

        let content = build_system_content(&identity, &tools, Some(""));
        assert!(
            !content.contains("OBSERVATION_LOG"),
            "empty observations should be skipped"
        );
    }

    #[test]
    fn system_content_skips_none_observations() {
        let identity = IdentityFiles::default();
        let tools = ToolRegistry::new();

        let content = build_system_content(&identity, &tools, None);
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
        let tools = ToolRegistry::new();
        let observations = "some observation";
        let content = build_system_content(&identity, &tools, Some(observations));

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
}
