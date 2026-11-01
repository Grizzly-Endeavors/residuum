//! System prompt assembly: combines identity, memory, and context into messages.

use residuum_inference::Message;
use residuum_workspace::identity::IdentityFiles;

use super::super::recent_messages::RecentMessages;
use super::prompt::build_system_prompt;
use super::types::{MemoryContext, PromptContext, RequestLayout};

/// Assemble the full message list for a model call, and what each part of
/// it is (its tool count left for the caller that adds the tools).
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
) -> (Vec<Message>, RequestLayout) {
    let conversation = recent_messages.messages();
    let status_lines = conversation
        .iter()
        .filter(|m| m.status_line.is_some())
        .count();
    let mut messages = Vec::with_capacity(1 + conversation.len() + status_lines);

    let system = build_system_prompt(identity, memory_ctx, prompt_ctx);
    messages.push(system.message);
    let mut layout = RequestLayout {
        system_sections: system.sections,
        ..RequestLayout::default()
    };
    for m in conversation {
        if let Some(line) = &m.status_line {
            layout.status_lines.insert(messages.len());
            messages.push(Message::system(line.clone()));
        }
        let mut rendered = m.clone();
        if m.sender.is_some() {
            rendered.content = m.attributed_content().into_owned();
        }
        messages.push(rendered);
    }

    (messages, layout)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;
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

        let (messages, _) =
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

        let (messages, _) =
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

        let (messages, _) =
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

        let (messages, layout) =
            assemble_system_prompt(&identity, &recent, &no_memory(), &PromptContext::default());

        assert_eq!(
            layout.status_lines.iter().copied().collect::<Vec<_>>(),
            [1, 5],
            "the layout names each stored line, and not a system message of the conversation"
        );
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

        let (messages, _) =
            assemble_system_prompt(&identity, &recent, &no_memory(), &PromptContext::default());
        assert_eq!(messages.len(), 2, "system prompt + user, no status line");
    }
}
