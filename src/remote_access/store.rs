//! What remote access keeps on disk besides the ACME account and the
//! certificate: the identity this install was enrolled under, the recovery
//! code until the person has saved it, and the pinned accounts this install
//! knows about.
//!
//! Lives at `hub/remote-access/state.json`, readable only by its owner.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

/// The user and instance slug this install enrolled or joined as. Every host
/// name is derived from these and the configured base domain; nothing the
/// relay announces replaces them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LocalIdentity {
    pub(crate) user: String,
    pub(crate) slug: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Persisted {
    #[serde(default)]
    identity: Option<LocalIdentity>,
    /// The pin recovery code, kept only until the person has saved it.
    #[serde(default)]
    pending_recovery_code: Option<String>,
    /// ACME account URIs this install pinned or approved.
    #[serde(default)]
    known_accounts: Vec<String>,
}

/// The state file, in memory and on disk.
pub(crate) struct StateStore {
    path: PathBuf,
    state: Mutex<Persisted>,
    /// Serializes saves so an older snapshot never overwrites a newer one.
    write_lock: tokio::sync::Mutex<()>,
}

impl StateStore {
    /// Open the store in `dir` (created on first save). A file that can't be
    /// read or parsed is moved aside and logged at `error`: this install then
    /// starts unenrolled, which is safe, because nothing is served remotely
    /// until it enrolls or joins again.
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

    pub(crate) fn is_known(&self, account_uri: &str) -> bool {
        self.lock().known_accounts.iter().any(|a| a == account_uri)
    }

    /// Store the identity and mark `own_account` as known, then save.
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

    /// Keep `code` until the person has saved it.
    pub(crate) async fn set_pending_recovery_code(
        &self,
        code: Option<String>,
    ) -> anyhow::Result<()> {
        self.lock().pending_recovery_code = code;
        self.save().await
    }

    /// Forget the accounts and identity of a previous enrollment, as a pin
    /// reset replaces them. `own_account` stays known.
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
        crate::util::fs::atomic_write_owner_only(&self.path, text)
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

    #[test]
    fn an_unparsable_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state.json"), "{ nope").unwrap();
        let store = StateStore::open(dir.path());
        assert!(store.identity().is_none());
        assert!(dir.path().join("state.json.unreadable").exists());
    }
}
