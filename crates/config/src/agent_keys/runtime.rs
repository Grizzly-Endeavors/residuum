use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{Mutex, RwLock};

use super::{AgentKeyError, AgentKeyStore, ENCRYPTED_FILE, KeyCreator, LOCK_FILE, Redactor};

pub type SharedAgentKeys = Arc<AgentKeys>;

#[derive(Debug, Default)]
pub struct AgentKeysSnapshot {
    pub store: AgentKeyStore,
    pub redactor: Redactor,
}

impl AgentKeysSnapshot {
    fn new(store: AgentKeyStore) -> Self {
        let redactor = Redactor::from_entries(store.entries());
        Self { store, redactor }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Debug, Default)]
struct Cache {
    stamp: Option<FileStamp>,
    primed: bool,
    snapshot: Arc<AgentKeysSnapshot>,
    load_error: Option<AgentKeyError>,
}

#[derive(Debug)]
pub struct AgentKeys {
    config_dir: PathBuf,
    cache: RwLock<Cache>,
    write_lock: Mutex<()>,
}

impl AgentKeys {
    #[must_use]
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
            cache: RwLock::new(Cache::default()),
            write_lock: Mutex::new(()),
        }
    }

    #[must_use]
    pub fn new_shared(config_dir: impl Into<PathBuf>) -> SharedAgentKeys {
        Arc::new(Self::new(config_dir))
    }

    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub async fn snapshot(&self) -> Result<Arc<AgentKeysSnapshot>, AgentKeyError> {
        let stamp = self.read_stamp().await;
        {
            let cache = self.cache.read().await;
            if cache.primed && cache.stamp == stamp {
                return match &cache.load_error {
                    Some(e) => Err(e.clone()),
                    None => Ok(Arc::clone(&cache.snapshot)),
                };
            }
        }

        let mut cache = self.cache.write().await;
        let fresh_stamp = self.read_stamp().await;
        if cache.primed && cache.stamp == fresh_stamp {
            return match &cache.load_error {
                Some(e) => Err(e.clone()),
                None => Ok(Arc::clone(&cache.snapshot)),
            };
        }

        cache.primed = true;
        cache.stamp = fresh_stamp;
        match self.load_blocking().await {
            Ok(store) => {
                tracing::debug!(count = store.names().len(), "agent keys loaded");
                cache.snapshot = Arc::new(AgentKeysSnapshot::new(store));
                cache.load_error = None;
                Ok(Arc::clone(&cache.snapshot))
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    config_dir = %self.config_dir.display(),
                    "failed to load agent keys; keeping the last good redaction set"
                );
                cache.load_error = Some(e.clone());
                Err(e)
            }
        }
    }

    pub async fn redactor(&self) -> Redactor {
        match self.snapshot().await {
            Ok(snapshot) => snapshot.redactor.clone(),
            Err(_) => self.cache.read().await.snapshot.redactor.clone(),
        }
    }

    pub async fn set(
        &self,
        name: &str,
        value: &str,
        description: Option<&str>,
        creator: KeyCreator,
    ) -> Result<Option<String>, AgentKeyError> {
        let name = name.to_string();
        let value = value.to_string();
        let description = description.map(str::to_string);
        self.mutate(move |store| store.set(&name, &value, description.as_deref(), creator))
            .await
    }

    pub async fn delete(&self, name: &str) -> Result<(), AgentKeyError> {
        let name = name.to_string();
        self.mutate(move |store| store.delete(&name)).await
    }

    async fn mutate<F, T>(&self, change: F) -> Result<T, AgentKeyError>
    where
        F: FnOnce(&mut AgentKeyStore) -> Result<T, AgentKeyError> + Send + 'static,
        T: Send + 'static,
    {
        let _guard = self.write_lock.lock().await;
        let config_dir = self.config_dir.clone();
        let (store, result) = residuum_core::util::spawn_blocking_in_span(move || {
            let _file_lock = lock_store_file(&config_dir)?;
            let mut store = AgentKeyStore::load(&config_dir)?;
            let result = change(&mut store)?;
            store.save(&config_dir)?;
            Ok::<_, AgentKeyError>((store, result))
        })
        .await
        .map_err(|e| AgentKeyError::Storage(format!("agent key write task failed: {e}")))??;

        let stamp = self.read_stamp().await;
        let mut cache = self.cache.write().await;
        cache.primed = true;
        cache.stamp = stamp;
        cache.snapshot = Arc::new(AgentKeysSnapshot::new(store));
        cache.load_error = None;
        Ok(result)
    }

    async fn load_blocking(&self) -> Result<AgentKeyStore, AgentKeyError> {
        let config_dir = self.config_dir.clone();
        residuum_core::util::spawn_blocking_in_span(move || AgentKeyStore::load(&config_dir))
            .await
            .map_err(|e| AgentKeyError::Storage(format!("agent key load task failed: {e}")))?
    }

    async fn read_stamp(&self) -> Option<FileStamp> {
        let path = self.config_dir.join(ENCRYPTED_FILE);
        tokio::fs::metadata(&path).await.ok().map(|m| FileStamp {
            modified: m.modified().ok(),
            len: m.len(),
        })
    }
}

fn lock_store_file(config_dir: &Path) -> Result<std::fs::File, AgentKeyError> {
    std::fs::create_dir_all(config_dir).map_err(|e| {
        AgentKeyError::Storage(format!(
            "failed to create config directory {}: {e}",
            config_dir.display()
        ))
    })?;
    let path = config_dir.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| {
            AgentKeyError::Storage(format!(
                "failed to open agent keys lock file at {}: {e}",
                path.display()
            ))
        })?;
    file.lock().map_err(|e| {
        AgentKeyError::Storage(format!(
            "failed to lock agent keys lock file at {}: {e}",
            path.display()
        ))
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_dir_gives_empty_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new(dir.path());
        let snap = keys.snapshot().await.unwrap();
        assert!(snap.store.names().is_empty(), "no file means no keys");
        assert!(snap.redactor.is_empty(), "no keys means nothing to redact");
    }

    #[tokio::test]
    async fn set_updates_snapshot_and_redactor() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new(dir.path());
        keys.set("api", "value-abcdefgh", Some("d"), KeyCreator::User)
            .await
            .unwrap();
        let snap = keys.snapshot().await.unwrap();
        assert_eq!(
            snap.store.value("api"),
            Some("value-abcdefgh"),
            "set should be visible immediately"
        );
        assert_eq!(
            keys.redactor().await.redact("x value-abcdefgh"),
            "x [agent-key:api]",
            "redactor should cover the new key"
        );
    }

    #[tokio::test]
    async fn external_write_is_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new(dir.path());
        assert!(
            keys.snapshot().await.unwrap().store.names().is_empty(),
            "starts empty"
        );

        let mut store = AgentKeyStore::load(dir.path()).unwrap();
        store
            .set("cli_key", "from-the-cli-123", None, KeyCreator::User)
            .unwrap();
        store.save(dir.path()).unwrap();

        let snap = keys.snapshot().await.unwrap();
        assert_eq!(
            snap.store.value("cli_key"),
            Some("from-the-cli-123"),
            "a write from another process should be reloaded"
        );
    }

    #[tokio::test]
    async fn corrupt_file_errors_but_redactor_keeps_last_good_set() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new(dir.path());
        keys.set("api", "value-abcdefgh", None, KeyCreator::User)
            .await
            .unwrap();

        std::fs::write(
            dir.path().join(ENCRYPTED_FILE),
            b"garbage-that-is-not-valid",
        )
        .unwrap();

        let err = keys.snapshot().await.unwrap_err();
        assert!(
            matches!(err, AgentKeyError::Storage(_)),
            "corrupt file should surface a storage error: {err}"
        );
        assert_eq!(
            keys.redactor().await.redact("value-abcdefgh"),
            "[agent-key:api]",
            "redaction must keep working from the last good set"
        );
    }

    #[tokio::test]
    async fn delete_removes_a_key_the_user_created() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new(dir.path());
        keys.set("mine", "value-abcdefgh", None, KeyCreator::User)
            .await
            .unwrap();
        keys.delete("mine").await.unwrap();
        assert!(
            keys.snapshot().await.unwrap().store.value("mine").is_none(),
            "the agent may delete a key the user created"
        );
    }
}
