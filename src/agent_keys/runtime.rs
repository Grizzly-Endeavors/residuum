//! Shared runtime handle over the agent key store.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{Mutex, RwLock};

use super::{AgentKeyError, AgentKeyStore, ENCRYPTED_FILE, KeyCreator, LOCK_FILE, Redactor};

/// Shared handle to the agent keys runtime.
pub type SharedAgentKeys = Arc<AgentKeys>;

/// A consistent view of the store and the redactor built from it.
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

/// What the file looked like when the cache was filled; any change means
/// another process (the CLI, a second gateway) wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Debug, Default)]
struct Cache {
    /// Stamp of the file the cache reflects; `None` = file absent.
    stamp: Option<FileStamp>,
    /// Whether `stamp` has been read at least once.
    primed: bool,
    snapshot: Arc<AgentKeysSnapshot>,
    /// Load failure for the current stamp, reported on every access but
    /// logged only when first seen.
    load_error: Option<AgentKeyError>,
}

/// Runtime agent-key store: cached, reloaded when the file changes on disk,
/// with serialized writes.
#[derive(Debug)]
pub struct AgentKeys {
    config_dir: PathBuf,
    cache: RwLock<Cache>,
    write_lock: Mutex<()>,
}

impl AgentKeys {
    /// A handle over the store in `config_dir`. Nothing is read until first use.
    #[must_use]
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
            cache: RwLock::new(Cache::default()),
            write_lock: Mutex::new(()),
        }
    }

    /// [`new`](Self::new), wrapped for sharing.
    #[must_use]
    pub fn new_shared(config_dir: impl Into<PathBuf>) -> SharedAgentKeys {
        Arc::new(Self::new(config_dir))
    }

    /// The config directory holding the store.
    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// The current store, reloaded first if the file changed on disk.
    ///
    /// # Errors
    /// Returns `AgentKeyError::Storage` if the file changed and can't be
    /// read or decrypted.
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
        // Another task may have reloaded while we waited for the write lock.
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

    /// The redactor for the current store. When the store can't be loaded,
    /// falls back to the last successfully loaded set (the failure is logged
    /// by [`snapshot`](Self::snapshot)) — redaction must never be skipped
    /// because of a reload error.
    pub async fn redactor(&self) -> Redactor {
        match self.snapshot().await {
            Ok(snapshot) => snapshot.redactor.clone(),
            Err(_) => self.cache.read().await.snapshot.redactor.clone(),
        }
    }

    /// Store a key and persist it.
    ///
    /// Reloads from disk under both the in-process lock and an exclusive
    /// lock on `agent-keys.lock` first, so a concurrent write from the CLI,
    /// the web API, or another handle isn't lost.
    ///
    /// # Errors
    /// Returns the store's validation or ownership error, or
    /// `AgentKeyError::Storage` if reading or writing the file fails.
    pub async fn set(
        &self,
        name: &str,
        value: &str,
        description: Option<&str>,
        creator: KeyCreator,
    ) -> Result<(), AgentKeyError> {
        let name = name.to_string();
        let value = value.to_string();
        let description = description.map(str::to_string);
        self.mutate(move |store| store.set(&name, &value, description.as_deref(), creator))
            .await
    }

    /// Delete a key and persist.
    ///
    /// # Errors
    /// Returns `AgentKeyError::NotFound`, `AgentKeyError::OwnedByUser`, or
    /// `AgentKeyError::Storage`.
    pub async fn delete(&self, name: &str, requester: KeyCreator) -> Result<(), AgentKeyError> {
        let name = name.to_string();
        self.mutate(move |store| store.delete(&name, requester))
            .await
    }

    async fn mutate<F>(&self, change: F) -> Result<(), AgentKeyError>
    where
        F: FnOnce(&mut AgentKeyStore) -> Result<(), AgentKeyError> + Send + 'static,
    {
        let _guard = self.write_lock.lock().await;
        let config_dir = self.config_dir.clone();
        let store = tokio::task::spawn_blocking(move || {
            let _file_lock = lock_store_file(&config_dir)?;
            let mut store = AgentKeyStore::load(&config_dir)?;
            change(&mut store)?;
            store.save(&config_dir)?;
            Ok::<_, AgentKeyError>(store)
        })
        .await
        .map_err(|e| AgentKeyError::Storage(format!("agent key write task failed: {e}")))??;

        let stamp = self.read_stamp().await;
        let mut cache = self.cache.write().await;
        cache.primed = true;
        cache.stamp = stamp;
        cache.snapshot = Arc::new(AgentKeysSnapshot::new(store));
        cache.load_error = None;
        Ok(())
    }

    async fn load_blocking(&self) -> Result<AgentKeyStore, AgentKeyError> {
        let config_dir = self.config_dir.clone();
        tokio::task::spawn_blocking(move || AgentKeyStore::load(&config_dir))
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

/// Take an exclusive lock on `agent-keys.lock` in `config_dir`, held until
/// the returned file is dropped. Serializes read-modify-write cycles across
/// processes (the CLI and the gateway) as well as across handles.
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

        // Simulates `residuum agent-keys set` from another process.
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
    async fn agent_delete_of_user_key_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let keys = AgentKeys::new(dir.path());
        keys.set("mine", "value-abcdefgh", None, KeyCreator::User)
            .await
            .unwrap();
        let err = keys.delete("mine", KeyCreator::Agent).await.unwrap_err();
        assert_eq!(
            err,
            AgentKeyError::OwnedByUser("mine".to_string()),
            "agent must not delete user keys"
        );
        assert!(
            keys.snapshot().await.unwrap().store.value("mine").is_some(),
            "key should survive the refused delete"
        );
    }
}
