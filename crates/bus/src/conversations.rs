use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use super::origin::ConversationKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownConversation {
    pub id: String,
    pub kind: ConversationKind,
    pub label: String,
}

#[async_trait]
pub trait ConversationSource: Send + Sync {
    async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>>;

    async fn owner_dm_conversation_id(&self) -> Option<String> {
        None
    }
}

type SourceMap = HashMap<String, Arc<dyn ConversationSource>>;

#[derive(Clone, Default)]
pub struct ConversationDirectory {
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

#[must_use = "the source is unregistered when this guard is dropped"]
pub struct Registration {
    directory: ConversationDirectory,
    endpoint: String,
    source: Arc<dyn ConversationSource>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut sources = self.directory.write();
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
        self.sources
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, SourceMap> {
        self.sources
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn register(&self, endpoint: &str, source: Arc<dyn ConversationSource>) -> Registration {
        self.write()
            .insert(endpoint.to_string(), Arc::clone(&source));
        Registration {
            directory: self.clone(),
            endpoint: endpoint.to_string(),
            source,
        }
    }

    #[must_use]
    pub fn endpoints(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    #[must_use]
    pub fn source(&self, endpoint: &str) -> Option<Arc<dyn ConversationSource>> {
        self.read().get(endpoint).cloned()
    }

    pub async fn owner_dm(&self, endpoint: &str) -> Option<String> {
        self.source(endpoint)?.owner_dm_conversation_id().await
    }

    pub async fn find(&self, endpoint: &str, id: &str) -> Result<KnownConversation, String> {
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
