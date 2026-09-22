//! Directory of the conversations each running chat interface can reach.
//!
//! Chat interfaces register a [`ConversationSource`] while they run; the
//! `list_conversations` and `send_message` tools read through the directory to
//! show the agent where it can post and to check a target before sending.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use super::chat_state::ConversationKind;

/// A conversation the agent can post into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownConversation {
    /// Interface-specific ID, as `send_message` takes it.
    pub(crate) id: String,
    /// Direct message, group chat, or channel.
    pub(crate) kind: ConversationKind,
    /// Human-readable place, e.g. `"#builds (Eng Team)"`.
    pub(crate) label: String,
}

/// Something that can say which conversations an interface can reach.
#[async_trait]
pub(crate) trait ConversationSource: Send + Sync {
    /// Every conversation the interface can currently post into.
    ///
    /// # Errors
    /// Returns an error if the interface has to ask its platform and that fails.
    async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>>;

    /// The conversation ID of the owner's direct message with the bot, if the
    /// owner has been claimed yet.
    ///
    /// Used by `send_message`'s session guard to tell whether a target
    /// conversation — or the no-conversation default — reaches the owner
    /// directly. The default implementation returns `None`, for sources with
    /// no owner concept.
    async fn owner_dm_conversation_id(&self) -> Option<String> {
        None
    }
}

type SourceMap = HashMap<String, Arc<dyn ConversationSource>>;

/// Shared, cheaply cloneable map from endpoint name to its conversation source.
#[derive(Clone, Default)]
pub(crate) struct ConversationDirectory {
    sources: Arc<RwLock<SourceMap>>,
}

impl std::fmt::Debug for ConversationDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names = self.endpoints();
        names.sort();
        f.debug_struct("ConversationDirectory")
            .field("endpoints", &names)
            .finish()
    }
}

/// Keeps an interface registered until dropped, so an adapter that stops or
/// restarts never leaves a stale source behind.
#[must_use = "the source is unregistered when this guard is dropped"]
pub(crate) struct Registration {
    directory: ConversationDirectory,
    endpoint: String,
    source: Arc<dyn ConversationSource>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut sources = self.directory.write();
        // A restarted adapter may already have replaced this registration.
        if sources
            .get(&self.endpoint)
            .is_some_and(|current| Arc::ptr_eq(current, &self.source))
        {
            sources.remove(&self.endpoint);
        }
    }
}

impl ConversationDirectory {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, SourceMap> {
        // Plain map of Arcs; a panic mid-insert cannot corrupt it.
        self.sources
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, SourceMap> {
        self.sources
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Register `source` for `endpoint` until the returned guard is dropped.
    pub(crate) fn register(
        &self,
        endpoint: &str,
        source: Arc<dyn ConversationSource>,
    ) -> Registration {
        self.write()
            .insert(endpoint.to_string(), Arc::clone(&source));
        Registration {
            directory: self.clone(),
            endpoint: endpoint.to_string(),
            source,
        }
    }

    /// Endpoints with a registered source.
    #[must_use]
    pub(crate) fn endpoints(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// The source for `endpoint`, if its interface is running.
    #[must_use]
    pub(crate) fn source(&self, endpoint: &str) -> Option<Arc<dyn ConversationSource>> {
        self.read().get(endpoint).cloned()
    }

    /// The owner's DM conversation ID on `endpoint`, if that endpoint has a
    /// running chat interface with a claimed owner.
    pub(crate) async fn owner_dm(&self, endpoint: &str) -> Option<String> {
        self.source(endpoint)?.owner_dm_conversation_id().await
    }

    /// Look up conversation `id` on `endpoint`, explaining in agent-facing
    /// terms why it cannot be used when it is not there.
    ///
    /// # Errors
    /// Returns a message if the endpoint has no running chat interface, its
    /// conversations cannot be listed, or none has this ID.
    pub(crate) async fn find(&self, endpoint: &str, id: &str) -> Result<KnownConversation, String> {
        let Some(source) = self.source(endpoint) else {
            return Err(format!(
                "'{endpoint}' has no conversations to choose from; only running chat \
                 interfaces (discord, telegram, teams) do — omit 'conversation' to send there"
            ));
        };
        let conversations = source.conversations().await.map_err(|e| {
            tracing::warn!(error = %e, endpoint, "failed to list conversations");
            format!("couldn't list conversations on '{endpoint}': {e}")
        })?;
        conversations
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| {
                format!(
                    "no conversation '{id}' on '{endpoint}'; use list_conversations to see \
                     the ones available"
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<KnownConversation>);

    #[async_trait]
    impl ConversationSource for Fixed {
        async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>> {
            Ok(self.0.clone())
        }
    }

    fn dm() -> KnownConversation {
        KnownConversation {
            id: "42".to_string(),
            kind: ConversationKind::Personal,
            label: "direct message with Bear".to_string(),
        }
    }

    #[tokio::test]
    async fn registered_source_is_listed_until_dropped() {
        let directory = ConversationDirectory::default();
        let guard = directory.register("discord", Arc::new(Fixed(vec![dm()])));
        let source = directory.source("discord").unwrap();
        assert_eq!(source.conversations().await.unwrap(), vec![dm()]);
        assert_eq!(directory.endpoints(), vec!["discord".to_string()]);

        drop(guard);
        assert!(directory.source("discord").is_none());
    }

    #[tokio::test]
    async fn find_explains_each_failure() {
        let directory = ConversationDirectory::default();
        let _guard = directory.register("discord", Arc::new(Fixed(vec![dm()])));

        assert_eq!(directory.find("discord", "42").await, Ok(dm()));
        let missing = directory.find("discord", "7").await.unwrap_err();
        assert!(missing.contains("list_conversations"), "{missing}");
        let no_source = directory.find("ws", "42").await.unwrap_err();
        assert!(no_source.contains("omit 'conversation'"), "{no_source}");
    }

    #[test]
    fn dropping_a_replaced_registration_keeps_the_new_one() {
        let directory = ConversationDirectory::default();
        let old = directory.register("teams", Arc::new(Fixed(vec![])));
        let _new = directory.register("teams", Arc::new(Fixed(vec![dm()])));
        drop(old);
        assert!(
            directory.source("teams").is_some(),
            "the restarted adapter's source must survive the old guard"
        );
    }
}
