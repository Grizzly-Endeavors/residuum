use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LocalIdentity {
    pub(crate) user: String,
    pub(crate) slug: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EmailReset {
    pub(crate) requested_at: i64,
    #[serde(default)]
    pub(crate) effective_at: Option<i64>,
    #[serde(default)]
    pub(crate) was_pinned: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Persisted {
    #[serde(default)]
    identity: Option<LocalIdentity>,
    #[serde(default)]
    pending_recovery_code: Option<String>,
    #[serde(default)]
    email_reset: Option<EmailReset>,
    #[serde(default)]
    known_accounts: Vec<String>,
}

pub(crate) struct StateStore {
    path: PathBuf,
    state: Mutex<Persisted>,
    write_lock: tokio::sync::Mutex<()>,
}

impl StateStore {
    pub(crate) fn open(dir: &Path) -> Self {
        let path = dir.join("state.json");
        let state = load(&path);
        Self {
            path,
            state: Mutex::new(state),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Persisted> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn identity(&self) -> Option<LocalIdentity> {
        self.lock().identity.clone()
    }

    pub(crate) fn pending_recovery_code(&self) -> Option<String> {
        self.lock().pending_recovery_code.clone()
    }

    pub(crate) fn email_reset(&self) -> Option<EmailReset> {
        self.lock().email_reset
    }

    pub(crate) fn is_known(&self, account_uri: &str) -> bool {
        self.lock().known_accounts.iter().any(|a| a == account_uri)
    }

    pub(crate) async fn set_identity(
        &self,
        identity: LocalIdentity,
        own_account: &str,
    ) -> anyhow::Result<()> {
        {
            let mut state = self.lock();
            state.identity = Some(identity);
            if !state.known_accounts.iter().any(|a| a == own_account) {
                state.known_accounts.push(own_account.to_string());
            }
        }
        self.save().await
    }

    pub(crate) async fn add_known(&self, account_uri: &str) -> anyhow::Result<()> {
        {
            let mut state = self.lock();
            if !state.known_accounts.iter().any(|a| a == account_uri) {
                state.known_accounts.push(account_uri.to_string());
            }
        }
        self.save().await
    }

    pub(crate) async fn forget_known(&self, account_uri: &str) -> anyhow::Result<()> {
        self.lock().known_accounts.retain(|a| a != account_uri);
        self.save().await
    }

    pub(crate) async fn set_pending_recovery_code(
        &self,
        code: Option<String>,
    ) -> anyhow::Result<()> {
        self.lock().pending_recovery_code = code;
        self.save().await
    }

    pub(crate) async fn set_recovery_state(
        &self,
        code: Option<String>,
        email_reset: Option<EmailReset>,
    ) -> anyhow::Result<()> {
        {
            let mut state = self.lock();
            state.pending_recovery_code = code;
            state.email_reset = email_reset;
        }
        self.save().await
    }

    pub(crate) async fn set_email_reset_effective_at(
        &self,
        effective_at: Option<i64>,
    ) -> anyhow::Result<()> {
        {
            let mut state = self.lock();
            let Some(reset) = state.email_reset.as_mut() else {
                return Ok(());
            };
            if reset.effective_at == effective_at {
                return Ok(());
            }
            reset.effective_at = effective_at;
        }
        self.save().await
    }

    pub(crate) async fn reset_known(&self, own_account: &str) -> anyhow::Result<()> {
        self.lock().known_accounts = vec![own_account.to_string()];
        self.save().await
    }

    async fn save(&self) -> anyhow::Result<()> {
        let _guard = self.write_lock.lock().await;
        let snapshot = self.lock().clone();
        let text = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| anyhow::anyhow!("failed to serialize the remote access state: {e}"))?;
        if let Some(dir) = self.path.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", dir.display()))?;
        }
        residuum_core::util::fs::atomic_write_owner_only(&self.path, text)
            .await
            .map_err(|e| anyhow::anyhow!("failed to write {}: {e:#}", self.path.display()))
    }
}

fn load(path: &Path) -> Persisted {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Persisted::default(),
        Err(e) => {
            tracing::error!(error = %e, path = %path.display(), "couldn't read the remote access state; remote access starts unenrolled");
            return Persisted::default();
        }
    };
    match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(e) => {
            let aside = path.with_extension("json.unreadable");
            tracing::error!(error = %e, path = %path.display(), moved_to = %aside.display(), "the remote access state can't be parsed; moved it aside, so remote access starts unenrolled");
            if let Err(rename) = std::fs::rename(path, &aside) {
                tracing::error!(error = %rename, path = %path.display(), "couldn't move the unreadable remote access state aside");
            }
            Persisted::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn identity_and_known_accounts_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(dir.path());
        assert!(store.identity().is_none());
        store
            .set_identity(
                LocalIdentity {
                    user: "bear".into(),
                    slug: "laptop".into(),
                },
                "https://acme.test/acct/1",
            )
            .await
            .unwrap();
        store
            .set_pending_recovery_code(Some("ABCDEFGHIJKLMNOPQRST".into()))
            .await
            .unwrap();
        let reopened = StateStore::open(dir.path());
        assert_eq!(reopened.identity().unwrap().slug, "laptop");
        assert!(reopened.is_known("https://acme.test/acct/1"));
        assert_eq!(
            reopened.pending_recovery_code().as_deref(),
            Some("ABCDEFGHIJKLMNOPQRST")
        );
    }

    #[tokio::test]
    async fn a_pending_email_reset_survives_a_reopen_with_its_code() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(dir.path());
        store
            .set_recovery_state(
                Some("ABCDEFGHIJKLMNOPQRST".into()),
                Some(EmailReset {
                    requested_at: 100,
                    effective_at: None,
                    was_pinned: false,
                }),
            )
            .await
            .unwrap();
        store.set_email_reset_effective_at(Some(200)).await.unwrap();
        let reopened = StateStore::open(dir.path());
        assert_eq!(
            reopened.email_reset(),
            Some(EmailReset {
                requested_at: 100,
                effective_at: Some(200),
                was_pinned: false,
            })
        );
        assert_eq!(
            reopened.pending_recovery_code().as_deref(),
            Some("ABCDEFGHIJKLMNOPQRST")
        );
        reopened.set_recovery_state(None, None).await.unwrap();
        assert!(StateStore::open(dir.path()).email_reset().is_none());
    }

    #[tokio::test]
    async fn approved_siblings_are_known_until_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(dir.path());
        assert!(!store.is_known("https://acme.test/acct/2"));
        store.add_known("https://acme.test/acct/2").await.unwrap();
        store.add_known("https://acme.test/acct/2").await.unwrap();
        assert!(StateStore::open(dir.path()).is_known("https://acme.test/acct/2"));
        store
            .forget_known("https://acme.test/acct/2")
            .await
            .unwrap();
        assert!(!StateStore::open(dir.path()).is_known("https://acme.test/acct/2"));
    }

    #[test]
    fn an_unparsable_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state.json"), "{ nope").unwrap();
        let store = StateStore::open(dir.path());
        assert!(store.identity().is_none());
        assert!(dir.path().join("state.json.unreadable").exists());
    }
}
