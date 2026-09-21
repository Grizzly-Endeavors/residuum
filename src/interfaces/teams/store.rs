//! Persisted Teams state: who owns the bot and how to reach each conversation.
//!
//! Proactive messages (scheduled results, `send_message`) need a stored
//! conversation reference because Teams gives the bot no way to open a chat
//! with nothing to go on. The owner is learned from the first direct message
//! and kept here so it survives restarts.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use super::activity::ConversationKind;

/// Everything needed to post into a conversation later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ConversationRef {
    pub(super) conversation_id: String,
    pub(super) service_url: String,
    pub(super) kind: ConversationKind,
    /// Human-readable location, e.g. `"#builds (Eng Team)"`.
    pub(super) label: String,
}

/// The person the bot answers to by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Owner {
    /// Entra object ID, stable across every chat the owner is in.
    pub(super) aad_object_id: String,
    pub(super) name: String,
    /// Conversation ID of the owner's direct message with the bot.
    pub(super) dm_conversation_id: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredState {
    owner: Option<Owner>,
    #[serde(default)]
    conversations: HashMap<String, ConversationRef>,
}

/// In-memory copy of the Teams state, written through to disk on change.
pub(super) struct TeamsStore {
    path: PathBuf,
    state: tokio::sync::Mutex<StoredState>,
}

impl TeamsStore {
    /// Load the store from `path`; a missing file starts empty.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub(super) async fn load(path: PathBuf) -> anyhow::Result<Self> {
        let state = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => serde_json::from_str(&contents)
                .with_context(|| format!("failed to parse teams state at {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoredState::default(),
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("failed to read teams state at {}", path.display())));
            }
        };
        Ok(Self {
            path,
            state: tokio::sync::Mutex::new(state),
        })
    }

    pub(super) async fn owner(&self) -> Option<Owner> {
        self.state.lock().await.owner.clone()
    }

    /// Record the owner if none is set yet. Returns whether this call set it.
    pub(super) async fn claim_owner(&self, owner: Owner) -> anyhow::Result<bool> {
        let mut state = self.state.lock().await;
        if state.owner.is_some() {
            return Ok(false);
        }
        state.owner = Some(owner);
        self.persist(&state).await?;
        Ok(true)
    }

    pub(super) async fn conversation(&self, conversation_id: &str) -> Option<ConversationRef> {
        self.state
            .lock()
            .await
            .conversations
            .get(conversation_id)
            .cloned()
    }

    /// Insert or update a conversation reference, writing only when it changed.
    pub(super) async fn remember(&self, conversation: ConversationRef) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        if state.conversations.get(&conversation.conversation_id) == Some(&conversation) {
            return Ok(());
        }
        state
            .conversations
            .insert(conversation.conversation_id.clone(), conversation);
        self.persist(&state).await
    }

    /// Forget a conversation the bot was removed from.
    pub(super) async fn forget(&self, conversation_id: &str) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        if state.conversations.remove(conversation_id).is_none() {
            return Ok(());
        }
        self.persist(&state).await
    }

    async fn persist(&self, state: &StoredState) -> anyhow::Result<()> {
        let json =
            serde_json::to_string_pretty(state).context("failed to serialize teams state")?;
        crate::util::fs::atomic_write(&self.path, &json)
            .await
            .with_context(|| format!("failed to write teams state at {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm(id: &str) -> ConversationRef {
        ConversationRef {
            conversation_id: id.to_string(),
            service_url: "https://smba.trafficmanager.net/amer/".to_string(),
            kind: ConversationKind::Personal,
            label: "direct message".to_string(),
        }
    }

    fn owner() -> Owner {
        Owner {
            aad_object_id: "aad-bear".to_string(),
            name: "Bear".to_string(),
            dm_conversation_id: "a:dm".to_string(),
        }
    }

    #[tokio::test]
    async fn owner_and_conversations_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams_state.json");

        let store = TeamsStore::load(path.clone()).await.unwrap();
        assert!(store.claim_owner(owner()).await.unwrap());
        store.remember(dm("a:dm")).await.unwrap();

        let reloaded = TeamsStore::load(path).await.unwrap();
        assert_eq!(reloaded.owner().await, Some(owner()));
        assert_eq!(reloaded.conversation("a:dm").await, Some(dm("a:dm")));
    }

    #[tokio::test]
    async fn first_owner_wins() {
        let dir = tempfile::tempdir().unwrap();
        let store = TeamsStore::load(dir.path().join("teams_state.json"))
            .await
            .unwrap();
        assert!(store.claim_owner(owner()).await.unwrap());
        let impostor = Owner {
            aad_object_id: "aad-other".to_string(),
            ..owner()
        };
        assert!(!store.claim_owner(impostor).await.unwrap());
        assert_eq!(store.owner().await, Some(owner()));
    }

    #[tokio::test]
    async fn forget_removes_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let store = TeamsStore::load(dir.path().join("teams_state.json"))
            .await
            .unwrap();
        store.remember(dm("19:chat")).await.unwrap();
        store.forget("19:chat").await.unwrap();
        assert_eq!(store.conversation("19:chat").await, None);
    }

    #[tokio::test]
    async fn corrupt_state_is_an_error_not_a_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams_state.json");
        tokio::fs::write(&path, "{not json").await.unwrap();
        assert!(TeamsStore::load(path).await.is_err());
    }
}
