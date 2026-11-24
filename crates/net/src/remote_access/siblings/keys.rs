use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use rand::Rng as _;
use rand::distributions::Alphanumeric;
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};

const KEY_PREFIX: &str = "rsdm_sib_";
const KEY_RANDOM_LEN: usize = 32;

#[must_use]
pub fn issue_key() -> String {
    let random: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(KEY_RANDOM_LEN)
        .map(char::from)
        .collect();
    format!("{KEY_PREFIX}{random}")
}

#[must_use]
pub(crate) fn is_key_shaped(key: &str) -> bool {
    key.strip_prefix(KEY_PREFIX).is_some_and(|rest| {
        rest.len() == KEY_RANDOM_LEN && rest.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

#[must_use]
pub(crate) fn hash_key(key: &str) -> String {
    format!(
        "sha256:{}",
        hex::encode(digest(&SHA256, key.as_bytes()).as_ref())
    )
}

pub trait SiblingKeyVerifier: Send + Sync {
    fn verify(&self, token: &str) -> Option<String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoSiblings;

impl SiblingKeyVerifier for NoSiblings {
    fn verify(&self, _token: &str) -> Option<String> {
        None
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    slug: String,
    #[serde(default)]
    display_name: String,
    account_uri: String,
    outbound_key: String,
    inbound_hash: String,
}

impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Record")
            .field("slug", &self.slug)
            .field("account_uri", &self.account_uri)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Persisted {
    #[serde(default)]
    siblings: Vec<Record>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JoinedSibling {
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) account_uri: String,
}

pub struct NewSibling {
    pub slug: String,
    pub display_name: String,
    pub account_uri: String,
    pub outbound_key: String,
    pub inbound_key: String,
}

pub struct SiblingKeys {
    path: PathBuf,
    state: Mutex<Persisted>,
    write_lock: tokio::sync::Mutex<()>,
}

impl SiblingKeys {
    #[must_use]
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("siblings.json");
        Self {
            state: Mutex::new(load(&path)),
            path,
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Persisted> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn joined(&self) -> Vec<JoinedSibling> {
        let mut joined: Vec<JoinedSibling> = self
            .lock()
            .siblings
            .iter()
            .map(|r| JoinedSibling {
                slug: r.slug.clone(),
                display_name: r.display_name.clone(),
                account_uri: r.account_uri.clone(),
            })
            .collect();
        joined.sort_by(|a, b| a.slug.cmp(&b.slug));
        joined
    }

    pub(crate) fn outbound_key(&self, slug: &str) -> Option<String> {
        self.lock()
            .siblings
            .iter()
            .find(|r| r.slug == slug)
            .map(|r| r.outbound_key.clone())
    }

    pub(crate) fn slug_for_account(&self, account_uri: &str) -> Option<String> {
        self.lock()
            .siblings
            .iter()
            .find(|r| r.account_uri == account_uri)
            .map(|r| r.slug.clone())
    }

    pub async fn upsert(&self, sibling: NewSibling) -> anyhow::Result<()> {
        let record = Record {
            slug: sibling.slug,
            display_name: sibling.display_name,
            account_uri: sibling.account_uri,
            outbound_key: sibling.outbound_key,
            inbound_hash: hash_key(&sibling.inbound_key),
        };
        {
            let mut state = self.lock();
            state.siblings.retain(|r| r.slug != record.slug);
            state.siblings.push(record);
        }
        self.save().await
    }

    pub async fn remove(&self, slug: &str) -> anyhow::Result<bool> {
        let removed = {
            let mut state = self.lock();
            let before = state.siblings.len();
            state.siblings.retain(|r| r.slug != slug);
            state.siblings.len() != before
        };
        if removed {
            self.save().await?;
        }
        Ok(removed)
    }

    async fn save(&self) -> anyhow::Result<()> {
        let _guard = self.write_lock.lock().await;
        let snapshot = self.lock().clone();
        let text = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| anyhow::anyhow!("failed to serialize the sibling keys: {e}"))?;
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

impl SiblingKeyVerifier for SiblingKeys {
    fn verify(&self, token: &str) -> Option<String> {
        if !is_key_shaped(token) {
            return None;
        }
        let candidate = hash_key(token);
        self.lock().siblings.iter().fold(None, |found, record| {
            let matches = residuum_core::util::secrets_match(&candidate, &record.inbound_hash);
            found.or(matches.then(|| record.slug.clone()))
        })
    }
}

impl std::fmt::Debug for SiblingKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiblingKeys")
            .field("joined", &self.joined())
            .finish_non_exhaustive()
    }
}

fn load(path: &Path) -> Persisted {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Persisted::default(),
        Err(e) => {
            tracing::error!(error = %e, path = %path.display(), "couldn't read the sibling keys; no sibling is trusted until it joins again");
            return Persisted::default();
        }
    };
    match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(e) => {
            let aside = path.with_extension("json.unreadable");
            tracing::error!(error = %e, path = %path.display(), moved_to = %aside.display(), "the sibling keys can't be parsed; moved them aside, so no sibling is trusted until it joins again");
            if let Err(rename) = std::fs::rename(path, &aside) {
                tracing::error!(error = %rename, path = %path.display(), "couldn't move the unreadable sibling keys aside");
            }
            Persisted::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_sibling(slug: &str) -> NewSibling {
        NewSibling {
            slug: slug.to_string(),
            display_name: "Desk".to_string(),
            account_uri: format!("https://acme.test/acct/{slug}"),
            outbound_key: issue_key(),
            inbound_key: issue_key(),
        }
    }

    #[test]
    fn issued_keys_have_the_documented_shape() {
        let key = issue_key();
        assert!(is_key_shaped(&key), "{key}");
        assert_ne!(key, issue_key());
        assert!(!is_key_shaped("rsdm_a2a_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(!is_key_shaped("rsdm_sib_short"));
    }

    #[tokio::test]
    async fn an_inbound_key_is_tagged_with_its_sibling_and_survives_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let keys = SiblingKeys::open(dir.path());
        let sibling = new_sibling("desktop");
        let inbound = sibling.inbound_key.clone();
        let outbound = sibling.outbound_key.clone();
        keys.upsert(sibling).await.unwrap();
        assert_eq!(keys.verify(&inbound).as_deref(), Some("desktop"));
        assert_eq!(
            keys.verify(&outbound),
            None,
            "the key we present isn't one we accept"
        );
        assert_eq!(keys.verify(&issue_key()), None);

        let reopened = SiblingKeys::open(dir.path());
        assert_eq!(reopened.verify(&inbound).as_deref(), Some("desktop"));
        assert_eq!(
            reopened.outbound_key("desktop").as_deref(),
            Some(outbound.as_str())
        );
        let raw = std::fs::read_to_string(dir.path().join("siblings.json")).unwrap();
        assert!(
            !raw.contains(&inbound),
            "an accepted key is stored only as a hash"
        );
    }

    #[tokio::test]
    async fn rejoining_replaces_the_old_keys_and_removal_ends_both() {
        let dir = tempfile::tempdir().unwrap();
        let keys = SiblingKeys::open(dir.path());
        let first = new_sibling("desktop");
        let old_inbound = first.inbound_key.clone();
        keys.upsert(first).await.unwrap();
        let second = new_sibling("desktop");
        let new_inbound = second.inbound_key.clone();
        keys.upsert(second).await.unwrap();
        assert_eq!(keys.joined().len(), 1);
        assert_eq!(keys.verify(&old_inbound), None);
        assert_eq!(keys.verify(&new_inbound).as_deref(), Some("desktop"));

        assert!(keys.remove("desktop").await.unwrap());
        assert_eq!(keys.verify(&new_inbound), None);
        assert_eq!(keys.outbound_key("desktop"), None);
        assert!(!keys.remove("desktop").await.unwrap());
    }

    #[tokio::test]
    async fn the_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let keys = SiblingKeys::open(dir.path());
        keys.upsert(new_sibling("desktop")).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("siblings.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn an_unparsable_file_is_moved_aside_and_trusts_nobody() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("siblings.json"), "{ nope").unwrap();
        let keys = SiblingKeys::open(dir.path());
        let actual = &keys.joined();
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
        assert!(dir.path().join("siblings.json.unreadable").exists());
    }

    #[test]
    fn debug_output_never_prints_a_key() {
        let keys = SiblingKeys::open(tempfile::tempdir().unwrap().path());
        assert!(!format!("{keys:?}").contains("rsdm_sib_"));
    }
}
