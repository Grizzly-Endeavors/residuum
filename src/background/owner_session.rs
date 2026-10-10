//! Sessions the owner starts by hand: a clean session from the web UI's
//! Activity, or a fork of the main conversation with `/multitask`.
//!
//! Either is an ordinary `spawned` session, the same as one main starts with
//! `subagent_spawn`, at depth 1 and with no spawner. Having no spawner is what
//! keeps its results out of the main conversation: the owner reads them in
//! the session itself (or, for a fork started from a chat app, in that chat).

use std::path::Path;

use anyhow::Context;

use crate::bus::{ConversationTarget, EventTrigger, SpawnRequestEvent};
use crate::config::BackgroundModelTier;
use crate::inference::Message;
use crate::memory::recent_messages::load_recent_messages;

use super::registry::{MAIN_DEPTH, generate_address};

/// What the owner asked for when starting a session.
pub(crate) struct OwnerSessionStart {
    /// The task, the session's first message.
    pub prompt: String,
    pub model_tier: BackgroundModelTier,
    /// Main's saved conversation, for a fork of it (`None` for a clean
    /// session). Empty when main's conversation has all been observed into
    /// memory, which the fork receives the same way main does.
    pub fork_of: Option<Vec<Message>>,
    /// The chat a fork started from a chat app replies into. `None` for a
    /// session started from the web UI, whose replies stay in its own panel.
    pub conversation: Option<ConversationTarget>,
}

impl OwnerSessionStart {
    /// The spawn request that starts this session.
    #[must_use]
    pub(crate) fn into_spawn_event(self) -> SpawnRequestEvent {
        let (qualifier, source_label, context) = if self.fork_of.is_some() {
            (
                "multitask",
                "owner:multitask",
                "[You are a fork of the main conversation: the messages before this one are that \
                 conversation. The owner started you with /multitask to work on the task below \
                 while the main conversation carries on separately. Main doesn't see your \
                 responses; the owner does.]",
            )
        } else {
            (
                "session",
                "owner:session",
                "[This session was started by the owner. Your responses are shown to them, \
                 not to the main conversation.]",
            )
        };
        SpawnRequestEvent {
            address: generate_address(&EventTrigger::Agent, qualifier),
            skill: None,
            source_label: source_label.to_string(),
            prompt: self.prompt,
            context: Some(context.to_string()),
            source: EventTrigger::Agent,
            model_tier: self.model_tier,
            spawner: None,
            depth: MAIN_DEPTH + 1,
            hop_count: 0,
            sender: None,
            conversation: self.conversation,
            inbound: None,
            images: Vec::new(),
            overlap: None,
            carried_history: self.fork_of.unwrap_or_default(),
        }
    }
}

/// Whether `event` starts (or resumes) a session the owner started by hand:
/// every `spawned` session an agent starts names its spawner.
#[must_use]
pub(crate) fn is_owner_start(event: &SpawnRequestEvent) -> bool {
    matches!(event.source, EventTrigger::Agent) && event.spawner.is_none()
}

/// Main's saved conversation (`recent_messages.json`), up to its last
/// finished turn, for a fork of it to start from.
///
/// # Errors
/// Returns an error if the file exists but can't be read or parsed.
pub(crate) async fn load_main_conversation(path: &Path) -> anyhow::Result<Vec<Message>> {
    Ok(load_recent_messages(path)
        .await
        .with_context(|| format!("failed to read main's conversation at {}", path.display()))?
        .into_iter()
        .map(|recent| recent.message)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::registry::SessionCategory;
    use crate::memory::recent_messages::append_recent_messages;
    use crate::memory::types::Visibility;

    fn start(fork_of: Option<Vec<Message>>) -> OwnerSessionStart {
        OwnerSessionStart {
            prompt: "look into the cabin booking".to_string(),
            model_tier: BackgroundModelTier::Large,
            fork_of,
            conversation: None,
        }
    }

    #[test]
    fn a_clean_session_is_a_spawned_session_with_no_spawner() {
        let event = start(None).into_spawn_event();

        assert_eq!(
            SessionCategory::from_trigger(&event.source),
            SessionCategory::Spawned
        );
        assert!(event.address.as_ref().starts_with("spawned-session-"));
        assert_eq!(event.spawner, None, "nothing relays its results to main");
        assert_eq!(event.depth, 1);
        assert_eq!(event.hop_count, 0);
        assert_eq!(event.model_tier, BackgroundModelTier::Large);
        assert_eq!(event.prompt, "look into the cabin booking");
        assert!(event.carried_history.is_empty());
    }

    #[test]
    fn a_fork_carries_main_s_conversation_and_says_what_it_is() {
        let event = start(Some(vec![Message::user("earlier")])).into_spawn_event();

        assert!(event.address.as_ref().starts_with("spawned-multitask-"));
        assert_eq!(event.source_label, "owner:multitask");
        assert_eq!(event.spawner, None);
        assert_eq!(event.carried_history.len(), 1);
        assert!(
            event
                .context
                .unwrap()
                .contains("fork of the main conversation")
        );
    }

    #[tokio::test]
    async fn loads_main_s_saved_conversation_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        append_recent_messages(
            &path,
            &[Message::user("first"), Message::assistant("second", None)],
            Visibility::User,
            chrono_tz::UTC,
            Some("turn-1"),
        )
        .await
        .unwrap();

        let messages = load_main_conversation(&path).await.unwrap();

        let texts: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(texts, ["first", "second"]);
    }

    #[test]
    fn a_fork_of_a_fully_observed_conversation_is_still_a_fork() {
        let event = start(Some(Vec::new())).into_spawn_event();

        assert!(event.address.as_ref().starts_with("spawned-multitask-"));
        assert!(event.carried_history.is_empty());
    }

    #[tokio::test]
    async fn a_conversation_with_no_saved_messages_forks_with_none() {
        let dir = tempfile::tempdir().unwrap();

        let messages = load_main_conversation(&dir.path().join("recent_messages.json"))
            .await
            .unwrap();

        assert!(messages.is_empty());
    }
}
