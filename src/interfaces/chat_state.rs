//! Persisted chat interface state: who owns the bot and how to reach each conversation.
//!
//! Each chat interface (Teams, Discord, Telegram) keeps one of these in its own
//! file in the workspace. The owner is learned from the first direct message
//! and kept so it survives restarts; conversation references let the agent
//! post into a chat later without having just heard from it.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::conversations::KnownConversation;
use super::types::ConversationKind;

/// A conversation reference for interfaces that reach a chat by its ID alone
/// (Discord channels, Telegram chats); the ID is the store key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChatRef {
    pub(crate) kind: ConversationKind,
    /// Human-readable location, e.g. `"direct message"`.
    pub(crate) label: String,
}

impl ChatRef {
    /// The reference for a 1:1 chat between `person` and the bot.
    pub(crate) fn direct_message(person: &str) -> Self {
        Self {
            kind: ConversationKind::Personal,
            label: direct_message_label(person),
        }
    }
}

/// How a 1:1 chat is labelled in `list_conversations`.
pub(crate) fn direct_message_label(person: &str) -> String {
    format!("direct message with {person}")
}

/// A stored conversation reference that can describe itself to the agent.
pub(crate) trait ConversationRecord {
    fn kind(&self) -> ConversationKind;
    fn label(&self) -> &str;
}

impl ConversationRecord for ChatRef {
    fn kind(&self) -> ConversationKind {
        self.kind
    }

    fn label(&self) -> &str {
        &self.label
    }
}

/// The person the bot answers to by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Owner {
    /// The platform's ID for the owner, stable across every chat they are in.
    #[serde(alias = "aad_object_id")]
    pub(crate) user_id: String,
    pub(crate) name: String,
    /// Conversation ID of the owner's direct message with the bot.
    pub(crate) dm_conversation_id: String,
}

/// Who sent a message, relative to the bot's owner.
pub(crate) enum Standing {
    Owner,
    Other { owner: Option<Owner> },
}

impl Standing {
    /// What to tell someone who is not allowed to use the bot.
    pub(crate) fn refusal(&self) -> Option<String> {
        match self {
            Self::Owner => None,
            Self::Other { owner: Some(owner) } => {
                Some(format!("I only take requests from {}.", owner.name))
            }
            Self::Other { owner: None } => Some(
                "I'm not set up yet. My owner needs to send me a direct message first.".to_string(),
            ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(bound(deserialize = "C: DeserializeOwned"))]
struct StoredState<C> {
    owner: Option<Owner>,
    #[serde(default = "HashMap::new")]
    conversations: HashMap<String, C>,
}

impl<C> Default for StoredState<C> {
    fn default() -> Self {
        Self {
            owner: None,
            conversations: HashMap::new(),
        }
    }
}

/// In-memory copy of one interface's state, written through to disk on change.
///
/// `C` is the interface's conversation reference: whatever it needs to post
/// into that conversation later.
pub(crate) struct ChatStateStore<C> {
    path: PathBuf,
    state: tokio::sync::Mutex<StoredState<C>>,
}

impl<C> ChatStateStore<C>
where
    C: Clone + PartialEq + Serialize + DeserializeOwned,
{
    /// Load the store from `path`; a missing file starts empty.
    ///
    /// A file that exists but fails to parse is moved aside (best-effort)
    /// rather than left to keep the adapter dead until someone manually
    /// fixes it — the store starts fresh instead, and the returned
    /// `Option<String>` names what happened for the caller to surface as
    /// a notice. A read failure other than "not found" (permissions, a
    /// bad mount) is still a hard error — that's not a corrupt-content
    /// problem this can self-heal.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read.
    pub(crate) async fn load(path: PathBuf) -> anyhow::Result<(Self, Option<String>)> {
        let (state, notice) = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(state) => (state, None),
                Err(parse_err) => {
                    let notice = Self::recover_from_corrupt_file(&path, &parse_err).await;
                    (StoredState::default(), Some(notice))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (StoredState::default(), None),
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!(
                    "failed to read chat interface state at {}",
                    path.display()
                )));
            }
        };
        Ok((
            Self {
                path,
                state: tokio::sync::Mutex::new(state),
            },
            notice,
        ))
    }

    /// Move a corrupt chat-state file aside (best-effort) and describe
    /// what happened, so a fresh, empty store can take its place instead
    /// of leaving the interface dead until someone fixes the file by hand.
    async fn recover_from_corrupt_file(
        path: &std::path::Path,
        parse_err: &serde_json::Error,
    ) -> String {
        let moved_aside = path.with_extension("json.corrupt");
        match tokio::fs::rename(path, &moved_aside).await {
            Ok(()) => {
                tracing::warn!(
                    path = %path.display(),
                    moved_to = %moved_aside.display(),
                    error = %parse_err,
                    "chat interface state was corrupt, moved aside and starting fresh"
                );
                format!(
                    "Your chat state file at {} was corrupt ({parse_err}) and has been moved to {} for reference. Starting fresh — whoever was recognized as the owner will need to message the bot again.",
                    path.display(),
                    moved_aside.display()
                )
            }
            Err(rename_err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %parse_err,
                    rename_error = %rename_err,
                    "chat interface state was corrupt and couldn't be moved aside, starting fresh anyway"
                );
                format!(
                    "Your chat state file at {} was corrupt ({parse_err}) and couldn't be moved aside ({rename_err}). Starting fresh — whoever was recognized as the owner will need to message the bot again.",
                    path.display()
                )
            }
        }
    }

    pub(crate) async fn owner(&self) -> Option<Owner> {
        self.state.lock().await.owner.clone()
    }

    /// Where `sender_id` stands relative to the owner.
    pub(crate) async fn standing_of(&self, sender_id: Option<&str>) -> Standing {
        let owner = self.owner().await;
        match (&owner, sender_id) {
            (Some(o), Some(id)) if o.user_id == id => Standing::Owner,
            _ => Standing::Other { owner },
        }
    }

    /// The sender's standing if they may use the bot, or the refusal to send
    /// them: only the owner is admitted unless `respond_to_others` is on.
    pub(crate) async fn admit(
        &self,
        sender_id: Option<&str>,
        respond_to_others: bool,
    ) -> Result<Standing, String> {
        let standing = self.standing_of(sender_id).await;
        match standing.refusal() {
            Some(refusal) if !respond_to_others => Err(refusal),
            _ => Ok(standing),
        }
    }

    /// Record the owner if none is set yet. Returns whether this call set it.
    ///
    /// # Errors
    /// Returns an error if the new owner cannot be written to disk.
    pub(crate) async fn claim_owner(&self, owner: Owner) -> anyhow::Result<bool> {
        let mut state = self.state.lock().await;
        if state.owner.is_some() {
            return Ok(false);
        }
        state.owner = Some(owner);
        self.persist(&state).await?;
        Ok(true)
    }

    pub(crate) async fn conversation(&self, conversation_id: &str) -> Option<C> {
        self.state
            .lock()
            .await
            .conversations
            .get(conversation_id)
            .cloned()
    }

    /// Every stored conversation, as the agent sees it in `list_conversations`.
    pub(crate) async fn known_conversations(&self) -> Vec<KnownConversation>
    where
        C: ConversationRecord,
    {
        self.state
            .lock()
            .await
            .conversations
            .iter()
            .map(|(id, c)| KnownConversation {
                id: id.clone(),
                kind: c.kind(),
                label: c.label().to_string(),
            })
            .collect()
    }

    /// Insert or update a conversation reference, writing only when it changed.
    ///
    /// # Errors
    /// Returns an error if the change cannot be written to disk.
    pub(crate) async fn remember(
        &self,
        conversation_id: &str,
        conversation: C,
    ) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        if state.conversations.get(conversation_id) == Some(&conversation) {
            return Ok(());
        }
        state
            .conversations
            .insert(conversation_id.to_string(), conversation);
        self.persist(&state).await
    }

    /// Forget a conversation the bot was removed from.
    ///
    /// # Errors
    /// Returns an error if the change cannot be written to disk.
    pub(crate) async fn forget(&self, conversation_id: &str) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        if state.conversations.remove(conversation_id).is_none() {
            return Ok(());
        }
        self.persist(&state).await
    }

    async fn persist(&self, state: &StoredState<C>) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .context("failed to serialize chat interface state")?;
        crate::util::fs::atomic_write(&self.path, &json)
            .await
            .with_context(|| {
                format!(
                    "failed to write chat interface state at {}",
                    self.path.display()
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Chat {
        label: String,
    }

    fn chat(label: &str) -> Chat {
        Chat {
            label: label.to_string(),
        }
    }

    fn owner() -> Owner {
        Owner {
            user_id: "u-bear".to_string(),
            name: "Bear".to_string(),
            dm_conversation_id: "dm".to_string(),
        }
    }

    async fn empty_store(dir: &tempfile::TempDir) -> ChatStateStore<Chat> {
        ChatStateStore::load(dir.path().join("state.json"))
            .await
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn owner_and_conversations_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        let (store, notice) = ChatStateStore::<Chat>::load(path.clone()).await.unwrap();
        assert!(
            notice.is_none(),
            "a fresh file should have nothing to report"
        );
        assert!(store.claim_owner(owner()).await.unwrap());
        store.remember("dm", chat("direct message")).await.unwrap();

        let (reloaded, reload_notice) = ChatStateStore::<Chat>::load(path).await.unwrap();
        assert!(
            reload_notice.is_none(),
            "a valid saved file should have nothing to report"
        );
        assert_eq!(reloaded.owner().await, Some(owner()));
        assert_eq!(
            reloaded.conversation("dm").await,
            Some(chat("direct message"))
        );
    }

    #[tokio::test]
    async fn corrupt_file_is_moved_aside_and_starts_fresh_with_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        tokio::fs::write(&path, "{ not valid json").await.unwrap();

        let (store, notice) = ChatStateStore::<Chat>::load(path.clone()).await.unwrap();
        let notice = notice.expect("a corrupt file should produce a notice");
        assert!(notice.contains(&path.display().to_string()), "{notice}");

        assert_eq!(store.owner().await, None, "should start with no owner");

        let moved_aside = path.with_extension("json.corrupt");
        assert!(moved_aside.exists(), "corrupt file should be moved aside");
        assert!(
            !path.exists(),
            "corrupt file should no longer be at the original path"
        );
        assert_eq!(
            tokio::fs::read_to_string(&moved_aside).await.unwrap(),
            "{ not valid json",
            "moved-aside copy should keep the original corrupt content"
        );
    }

    #[tokio::test]
    async fn missing_file_starts_empty_with_no_notice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        let (store, notice) = ChatStateStore::<Chat>::load(path).await.unwrap();
        assert!(notice.is_none(), "a missing file is not corruption");
        assert_eq!(store.owner().await, None);
    }

    #[tokio::test]
    async fn first_owner_wins() {
        let dir = tempfile::tempdir().unwrap();
        let store = empty_store(&dir).await;
        assert!(store.claim_owner(owner()).await.unwrap());
        let impostor = Owner {
            user_id: "u-other".to_string(),
            ..owner()
        };
        assert!(!store.claim_owner(impostor).await.unwrap());
        assert_eq!(store.owner().await, Some(owner()));
    }

    #[tokio::test]
    async fn standing_distinguishes_owner_from_others() {
        let dir = tempfile::tempdir().unwrap();
        let store = empty_store(&dir).await;
        let before = store.standing_of(Some("u-bear")).await;
        assert!(
            before
                .refusal()
                .is_some_and(|r| r.contains("send me a direct message first")),
            "no owner yet: everyone is refused with setup instructions"
        );

        store.claim_owner(owner()).await.unwrap();
        assert!(matches!(
            store.standing_of(Some("u-bear")).await,
            Standing::Owner
        ));
        let other = store.standing_of(Some("u-other")).await;
        assert_eq!(
            other.refusal().as_deref(),
            Some("I only take requests from Bear.")
        );
        assert!(store.standing_of(None).await.refusal().is_some());
    }

    #[tokio::test]
    async fn admit_turns_others_away_unless_respond_to_others() {
        let dir = tempfile::tempdir().unwrap();
        let store = empty_store(&dir).await;
        store.claim_owner(owner()).await.unwrap();

        assert!(matches!(
            store.admit(Some("u-bear"), false).await,
            Ok(Standing::Owner)
        ));
        assert_eq!(
            store.admit(Some("u-other"), false).await.err().as_deref(),
            Some("I only take requests from Bear.")
        );
        assert!(matches!(
            store.admit(Some("u-other"), true).await,
            Ok(Standing::Other { .. })
        ));
    }

    #[tokio::test]
    async fn teams_owner_field_name_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams_state.json");
        tokio::fs::write(
            &path,
            r#"{"owner":{"aad_object_id":"aad-bear","name":"Bear","dm_conversation_id":"a:dm"}}"#,
        )
        .await
        .unwrap();
        let (store, _notice) = ChatStateStore::<Chat>::load(path).await.unwrap();
        assert_eq!(
            store.owner().await.map(|o| o.user_id).as_deref(),
            Some("aad-bear")
        );
    }

    #[tokio::test]
    async fn forget_removes_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let store = empty_store(&dir).await;
        store.remember("19:chat", chat("group chat")).await.unwrap();
        store.forget("19:chat").await.unwrap();
        assert_eq!(store.conversation("19:chat").await, None);
    }
}
