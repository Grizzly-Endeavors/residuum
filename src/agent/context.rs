//! Context assembly: builds the system prompt from identity files and tool info.

use crate::models::Message;
use crate::models::Role;
use crate::tools::ToolRegistry;
use crate::workspace::identity::IdentityFiles;

use super::session::Session;

/// Assemble the full message list for a model call.
///
/// Creates a system message from identity files and tool listings,
/// then appends the session history.
#[must_use]
pub fn assemble_system_prompt(
    identity: &IdentityFiles,
    tools: &ToolRegistry,
    session: &Session,
) -> Vec<Message> {
    let system_content = build_system_content(identity, tools);

    let mut messages = Vec::with_capacity(1 + session.messages().len());

    messages.push(Message {
        role: Role::System,
        content: system_content,
        tool_calls: None,
        tool_call_id: None,
    });

    messages.extend(session.messages().iter().cloned());

    messages
}

/// Build the system prompt content from identity files and tool listings.
///
/// Assembly order:
/// 1. SOUL.md content
/// 2. AGENTS.md content
/// 3. TOOLS.md content
/// 4. Available tool names listing
/// 5. USER.md content
/// 6. MEMORY.md content
fn build_system_content(identity: &IdentityFiles, tools: &ToolRegistry) -> String {
    let mut parts = Vec::new();

    if let Some(soul) = &identity.soul {
        parts.push(soul.clone());
    }

    if let Some(agents) = &identity.agents {
        parts.push(agents.clone());
    }

    if let Some(tools_md) = &identity.tools {
        parts.push(tools_md.clone());
    }

    // List available tools
    let tool_defs = tools.definitions();
    if !tool_defs.is_empty() {
        let tool_names: Vec<&str> = tool_defs.iter().map(|t| t.name.as_str()).collect();
        parts.push(format!("## Available Tools\n\n{}", tool_names.join(", ")));
    }

    if let Some(user) = &identity.user {
        parts.push(user.clone());
    }

    if let Some(memory) = &identity.memory {
        parts.push(memory.clone());
    }

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_with_empty_identity() {
        let identity = IdentityFiles::default();
        let tools = ToolRegistry::new();
        let session = Session::new();

        let messages = assemble_system_prompt(&identity, &tools, &session);
        assert_eq!(messages.len(), 1, "should have system message only");
        assert_eq!(
            messages.first().map(|m| &m.role),
            Some(&Role::System),
            "first message should be system"
        );
    }

    #[test]
    fn assemble_includes_session_history() {
        let identity = IdentityFiles::default();
        let tools = ToolRegistry::new();
        let mut session = Session::new();
        session.push(Message {
            role: Role::User,
            content: "hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        });

        let messages = assemble_system_prompt(&identity, &tools, &session);
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

        let content = build_system_content(&identity, &tools);
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
    fn system_content_includes_tool_listing() {
        let identity = IdentityFiles::default();
        let mut tools = ToolRegistry::new();
        tools.register_defaults();

        let content = build_system_content(&identity, &tools);
        assert!(content.contains("read_file"), "should list read_file tool");
        assert!(
            content.contains("write_file"),
            "should list write_file tool"
        );
        assert!(content.contains("exec"), "should list exec tool");
    }
}
