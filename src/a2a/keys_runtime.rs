//! Shared runtime handle over the A2A caller-key store.
//!
//! Modeled on [`crate::agent_keys::AgentKeys`]: an in-process cache guarded
//! by an async mutex for writes, reloaded when the file's mtime/length
//! changes, and an OS file lock around the read-modify-write cycle so the
//! CLI, the web API, and the listener's auth layer never lose a concurrent
//! write to each other.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{Mutex, RwLock};

use super::keys::{A2aKeyError, A2aKeyStore, KEYS_FILE, LOCK_FILE};

/// Shared handle to the A2A caller-keys runtime.
pub type SharedA2aKeys = Arc<A2aKeys>;

/// What the file looked like when the cache was filled; any change means
/// another process (the CLI, a second gateway) wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Default)]
struct Cache {
    stamp: Option<FileStamp>,
    primed: bool,
    store: Arc<A2aKeyStore>,
    load_error: Option<A2aKeyError>,
}

/// Runtime A2A caller-key store: cached, reloaded when the file changes on
/// disk, with serialized writes.
pub struct A2aKeys {
    config_dir: PathBuf,
    cache: RwLock<Cache>,
    write_lock: Mutex<()>,
}

impl A2aKeys {
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
    pub fn new_shared(config_dir: impl Into<PathBuf>) -> SharedA2aKeys {
        Arc::new(Self::new(config_dir))
    }

    /// The current store, reloaded first if the file changed on disk.
    ///
    /// # Errors
    /// Returns `A2aKeyError::Storage` if the file changed and can't be read
    /// or parsed.
    pub async fn snapshot(&self) -> Result<Arc<A2aKeyStore>, A2aKeyError> {
        let stamp = self.read_stamp().await;
        {
            let cache = self.cache.read().await;
            if cache.primed && cache.stamp == stamp {
                return match &cache.load_error {
                    Some(e) => Err(e.clone()),
                    None => Ok(Arc::clone(&cache.store)),
                };
            }
        }

        let mut cache = self.cache.write().await;
        // Another task may have reloaded while we waited for the write lock.
        let fresh_stamp = self.read_stamp().await;
        if cache.primed && cache.stamp == fresh_stamp {
            return match &cache.load_error {
                Some(e) => Err(e.clone()),
                None => Ok(Arc::clone(&cache.store)),
            };
        }

        cache.primed = true;
        cache.stamp = fresh_stamp;
        match self.load_blocking().await {
            Ok(store) => {
                tracing::debug!(count = store.list().len(), "A2A caller keys loaded");
                cache.store = Arc::new(store);
                cache.load_error = None;
                Ok(Arc::clone(&cache.store))
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    config_dir = %self.config_dir.display(),
                    "failed to load A2A caller keys"
                );
                cache.load_error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// The caller name for `token`, or `None` if it matches no live key or
    /// the store can't currently be loaded. A load failure fails closed
    /// (denies the caller) rather than falling back to a stale key set,
    /// since a stale *allow* here would be a security regression, unlike a
    /// stale *redaction* set.
    pub async fn verify(&self, token: &str) -> Option<String> {
        self.snapshot().await.ok()?.verify(token)
    }

    /// Mint a new caller key and persist it, returning the token.
    ///
    /// Reloads from disk under both the in-process lock and an exclusive
    /// lock on `a2a-keys.lock` first, so a concurrent write from the CLI,
    /// the web API, or another handle isn't lost.
    ///
    /// # Errors
    /// Returns the store's validation error, or `A2aKeyError::Storage` if
    /// reading or writing the file fails.
    pub async fn create(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<String, A2aKeyError> {
        let name = name.to_string();
        let description = description.map(str::to_string);
        self.mutate(move |store| store.create(&name, description.as_deref()))
            .await
    }

    /// Revoke a caller key and persist.
    ///
    /// # Errors
    /// Returns `A2aKeyError::NotFound` or `A2aKeyError::Storage`.
    pub async fn revoke(&self, name: &str) -> Result<(), A2aKeyError> {
        let name = name.to_string();
        self.mutate(move |store| store.revoke(&name)).await
    }

    async fn mutate<F, R>(&self, change: F) -> Result<R, A2aKeyError>
    where
        F: FnOnce(&mut A2aKeyStore) -> Result<R, A2aKeyError> + Send + 'static,
        R: Send + 'static,
    {
        let _guard = self.write_lock.lock().await;
        let config_dir = self.config_dir.clone();
        let (store, result) = tokio::task::spawn_blocking(move || {
            let _file_lock = lock_store_file(&config_dir)?;
            let mut store = A2aKeyStore::load(&config_dir)?;
            let result = change(&mut store)?;
            store.save(&config_dir)?;
            Ok::<_, A2aKeyError>((store, result))
        })
        .await
        .map_err(|e| A2aKeyError::Storage(format!("A2A key write task failed: {e}")))??;

        let stamp = self.read_stamp().await;
        let mut cache = self.cache.write().await;
        cache.primed = true;
        cache.stamp = stamp;
        cache.store = Arc::new(store);
        cache.load_error = None;
        Ok(result)
    }

    async fn load_blocking(&self) -> Result<A2aKeyStore, A2aKeyError> {
        let config_dir = self.config_dir.clone();
        tokio::task::spawn_blocking(move || A2aKeyStore::load(&config_dir))
            .await
            .map_err(|e| A2aKeyError::Storage(format!("A2A key load task failed: {e}")))?
    }

    async fn read_stamp(&self) -> Option<FileStamp> {
        let path = self.config_dir.join(KEYS_FILE);
        tokio::fs::metadata(&path).await.ok().map(|m| FileStamp {
            modified: m.modified().ok(),
            len: m.len(),
        })
    }
}

/// Take an exclusive lock on `a2a-keys.lock` in `config_dir`, held until the
/// returned file is dropped. Serializes read-modify-write cycles across
/// processes (the CLI and the gateway) as well as across handles.
fn lock_store_file(config_dir: &Path) -> Result<std::fs::File, A2aKeyError> {
    std::fs::create_dir_all(config_dir).map_err(|e| {
        A2aKeyError::Storage(format!(
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
            A2aKeyError::Storage(format!(
                "failed to open A2A caller keys lock file at {}: {e}",
                path.display()
            ))
        })?;
    file.lock().map_err(|e| {
        A2aKeyError::Storage(format!(
            "failed to lock A2A caller keys lock file at {}: {e}",
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
        let keys = A2aKeys::new(dir.path());
        let snap = keys.snapshot().await.unwrap();
        assert!(snap.list().is_empty());
    }

    #[tokio::test]
    async fn create_is_visible_immediately_and_verifiable() {
        let dir = tempfile::tempdir().unwrap();
        let keys = A2aKeys::new(dir.path());
        let token = keys.create("laptop", Some("d")).await.unwrap();
        assert_eq!(keys.verify(&token).await, Some("laptop".to_string()));
    }

    #[tokio::test]
    async fn revoke_removes_verification() {
        let dir = tempfile::tempdir().unwrap();
        let keys = A2aKeys::new(dir.path());
        let token = keys.create("laptop", None).await.unwrap();
        keys.revoke("laptop").await.unwrap();
        assert_eq!(keys.verify(&token).await, None);
    }

    #[tokio::test]
    async fn external_write_is_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        let keys = A2aKeys::new(dir.path());
        assert!(keys.snapshot().await.unwrap().list().is_empty());

        // Simulates `residuum a2a keys create` from another process.
        let mut store = A2aKeyStore::load(dir.path()).unwrap();
        let token = store.create("cli_made", None).unwrap();
        store.save(dir.path()).unwrap();

        assert_eq!(keys.verify(&token).await, Some("cli_made".to_string()));
    }

    #[tokio::test]
    async fn corrupt_file_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let keys = A2aKeys::new(dir.path());
        let token = keys.create("laptop", None).await.unwrap();
        std::fs::write(dir.path().join(KEYS_FILE), b"not valid toml {{{").unwrap();

        assert_eq!(
            keys.verify(&token).await,
            None,
            "a load failure must deny, never fall back to a stale allow"
        );
    }

    #[tokio::test]
    async fn duplicate_create_does_not_lose_the_first_key() {
        let dir = tempfile::tempdir().unwrap();
        let keys = A2aKeys::new(dir.path());
        let first = keys.create("beta", None).await.unwrap();
        assert!(keys.create("beta", None).await.is_err());
        assert_eq!(keys.verify(&first).await, Some("beta".to_string()));
    }
}
