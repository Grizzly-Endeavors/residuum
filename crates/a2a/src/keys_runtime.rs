use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{Mutex, RwLock};

use super::keys::{A2aKeyError, A2aKeyStore, KEYS_FILE, LOCK_FILE};

pub type SharedA2aKeys = Arc<A2aKeys>;

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

pub struct A2aKeys {
    config_dir: PathBuf,
    cache: RwLock<Cache>,
    write_lock: Mutex<()>,
}

impl A2aKeys {
    #[must_use]
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
            cache: RwLock::new(Cache::default()),
            write_lock: Mutex::new(()),
        }
    }

    #[must_use]
    pub fn new_shared(config_dir: impl Into<PathBuf>) -> SharedA2aKeys {
        Arc::new(Self::new(config_dir))
    }

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

    pub async fn verify(&self, token: &str) -> Option<String> {
        self.snapshot().await.ok()?.verify(token)
    }

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
        let (store, result) = residuum_core::util::spawn_blocking_in_span(move || {
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
        residuum_core::util::spawn_blocking_in_span(move || A2aKeyStore::load(&config_dir))
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
        let actual = &snap.list();
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
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
        let actual_2 = &keys.snapshot().await.unwrap().list();
        assert!(actual_2.is_empty(), "expected {actual_2:?} to be empty");

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
