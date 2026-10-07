//! The pairing file: the identity Residuum Cloud announced and the paired
//! devices, as hashes.
//!
//! Lives at `hub/remote-access.json`, readable only by its owner. It holds no
//! credential that can be replayed: a device's cookie value is stored as its
//! SHA-256, and so is every unused recovery code.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::PairingError;

/// What Residuum Cloud announced about this install, kept so the pairing link,
/// the credential cookie's name and the cross-site check still work while the
/// tunnel is reconnecting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// This instance's slug, which names the device cookie.
    #[serde(default)]
    pub slug: Option<String>,
    /// The origin the web UI is reached at through the relay.
    #[serde(default)]
    pub ui_origin: Option<String>,
    /// The origin the workbench artifacts are reached at through the relay.
    #[serde(default)]
    pub workbench_origin: Option<String>,
}

/// A paired browser. The secrets are hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredDevice {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) created_at: DateTime<Utc>,
    pub(super) last_seen: DateTime<Utc>,
    /// When the cookie was last re-issued, so its lifetime keeps extending.
    #[serde(default)]
    pub(super) refreshed_at: Option<DateTime<Utc>>,
    /// Hash of the credential on the UI host.
    #[serde(default)]
    pub(super) ui_hash: Option<String>,
    /// Hashes of the credentials on the workbench host. A few can coexist,
    /// because several frames can complete a handoff at once.
    #[serde(default)]
    pub(super) workbench_hashes: Vec<String>,
}

/// The unused recovery codes, as hashes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RecoveryCodes {
    #[serde(default)]
    pub(super) generated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(super) hashes: Vec<String>,
}

/// Everything that survives a restart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Persisted {
    #[serde(default)]
    pub(super) identity: Identity,
    #[serde(default)]
    pub(super) devices: Vec<StoredDevice>,
    #[serde(default)]
    pub(super) recovery: RecoveryCodes,
}

/// Read the pairing file. A missing file is an empty store.
///
/// A file that can't be parsed is moved aside to `<name>.unreadable` and the
/// store starts empty: nobody can be let in on a file Residuum can't read, and
/// the person at the machine can pair again. The move and the reason are
/// logged at `error`.
pub(super) fn load(path: &Path) -> Persisted {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Persisted::default(),
        Err(e) => {
            tracing::error!(error = %e, path = %path.display(), "couldn't read the paired devices file; no device is paired until it can be read");
            return Persisted::default();
        }
    };
    match serde_json::from_str::<Persisted>(&text) {
        Ok(persisted) => persisted,
        Err(e) => {
            let aside = path.with_extension("json.unreadable");
            tracing::error!(error = %e, path = %path.display(), moved_to = %aside.display(), "the paired devices file can't be parsed; moved it aside, so every device must pair again");
            if let Err(rename) = std::fs::rename(path, &aside) {
                tracing::error!(error = %rename, path = %path.display(), "couldn't move the unreadable paired devices file aside");
            }
            Persisted::default()
        }
    }
}

/// Write the pairing file, readable only by its owner.
pub(super) async fn save(path: &Path, persisted: &Persisted) -> Result<(), PairingError> {
    let text = serde_json::to_string_pretty(persisted).map_err(|e| {
        tracing::error!(error = %e, "failed to serialize the paired devices");
        PairingError::Storage(format!("Residuum couldn't save the paired devices: {e}."))
    })?;
    crate::util::fs::atomic_write_owner_only(path, text)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, path = %path.display(), "failed to write the paired devices file");
            PairingError::Storage(format!(
                "Residuum couldn't save the paired devices to {}: {e:#}.",
                path.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_an_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("remote-access.json")),
            Persisted::default()
        );
    }

    #[tokio::test]
    async fn a_saved_store_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-access.json");
        let mut persisted = Persisted::default();
        persisted.identity.slug = Some("laptop".to_string());
        save(&path, &persisted).await.unwrap();
        assert_eq!(load(&path), persisted);
    }

    #[test]
    fn an_unparsable_file_is_moved_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-access.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load(&path), Persisted::default());
        assert!(!path.exists(), "the bad file must not stay in place");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("remote-access.json.unreadable")).unwrap(),
            "{ not json"
        );
    }
}
