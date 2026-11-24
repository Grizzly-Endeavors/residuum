use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rand::Rng;
use rand::distributions::Alphanumeric;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) const KEYS_FILE: &str = "a2a-keys.toml";

pub(crate) const LOCK_FILE: &str = "a2a-keys.lock";

const TOKEN_PREFIX: &str = "rsdm_a2a_";

const TOKEN_RANDOM_LEN: usize = 32;

const MAX_NAME_LEN: usize = 64;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum A2aKeyError {
    #[error("{0}")]
    Invalid(String),
    #[error("no A2A caller key named '{0}'")]
    NotFound(String),
    #[error("an A2A caller key named '{0}' already exists")]
    AlreadyExists(String),
    #[error("A2A caller key store unavailable: {0}")]
    Storage(String),
}

#[derive(Clone, Serialize, Deserialize)]
struct KeyEntry {
    #[serde(default)]
    description: String,
    hash: String,
    created_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Default)]
struct KeysFile {
    #[serde(default)]
    keys: BTreeMap<String, KeyEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct A2aKeyInfo {
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Default, Clone)]
pub struct A2aKeyStore {
    keys: BTreeMap<String, KeyEntry>,
}

impl std::fmt::Debug for A2aKeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A2aKeyStore")
            .field("names", &self.keys.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl A2aKeyStore {
    pub fn load(config_dir: &Path) -> Result<Self, A2aKeyError> {
        let path = config_dir.join(KEYS_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            A2aKeyError::Storage(format!(
                "failed to read A2A caller keys file at {}: {e}",
                path.display()
            ))
        })?;
        let file: KeysFile = toml::from_str(&raw).map_err(|e| {
            A2aKeyError::Storage(format!(
                "failed to parse A2A caller keys file at {}: {e}",
                path.display()
            ))
        })?;
        Ok(Self { keys: file.keys })
    }

    pub fn save(&self, config_dir: &Path) -> Result<(), A2aKeyError> {
        std::fs::create_dir_all(config_dir).map_err(|e| {
            A2aKeyError::Storage(format!(
                "failed to create config directory {}: {e}",
                config_dir.display()
            ))
        })?;
        let plaintext = toml::to_string_pretty(&KeysFile {
            keys: self.keys.clone(),
        })
        .map_err(|e| A2aKeyError::Storage(format!("failed to serialize A2A caller keys: {e}")))?;

        let path = config_dir.join(KEYS_FILE);
        let tmp_path: PathBuf = config_dir.join(format!("{KEYS_FILE}.tmp"));
        std::fs::write(&tmp_path, &plaintext).map_err(|e| {
            A2aKeyError::Storage(format!(
                "failed to write A2A caller keys file at {}: {e}",
                tmp_path.display()
            ))
        })?;
        set_owner_only(&tmp_path).map_err(|e| {
            A2aKeyError::Storage(format!(
                "failed to restrict permissions on {}: {e}",
                tmp_path.display()
            ))
        })?;
        std::fs::rename(&tmp_path, &path).map_err(|e| {
            A2aKeyError::Storage(format!(
                "failed to move A2A caller keys file into place at {}: {e}",
                path.display()
            ))
        })
    }

    #[must_use]
    pub fn list(&self) -> Vec<A2aKeyInfo> {
        self.keys
            .iter()
            .map(|(name, entry)| A2aKeyInfo {
                name: name.clone(),
                description: entry.description.clone(),
                created_at: entry.created_at,
            })
            .collect()
    }

    pub fn create(&mut self, name: &str, description: Option<&str>) -> Result<String, A2aKeyError> {
        validate_name(name)?;
        if self.keys.contains_key(name) {
            return Err(A2aKeyError::AlreadyExists(name.to_string()));
        }
        let token = generate_token();
        self.keys.insert(
            name.to_string(),
            KeyEntry {
                description: description.unwrap_or_default().trim().to_string(),
                hash: hash_token(&token),
                created_at: Utc::now(),
            },
        );
        Ok(token)
    }

    pub fn revoke(&mut self, name: &str) -> Result<(), A2aKeyError> {
        if self.keys.remove(name).is_none() {
            return Err(A2aKeyError::NotFound(name.to_string()));
        }
        Ok(())
    }

    #[must_use]
    pub fn verify(&self, token: &str) -> Option<String> {
        let candidate = hash_token(token);
        self.keys
            .iter()
            .find(|(_, entry)| residuum_core::util::secrets_match(&candidate, &entry.hash))
            .map(|(name, _)| name.clone())
    }
}

fn hash_token(token: &str) -> String {
    use std::fmt::Write as _;

    use ring::digest::{SHA256, digest};
    let bytes = digest(&SHA256, token.as_bytes());
    let mut hex = String::with_capacity(2 * bytes.as_ref().len() + 7);
    hex.push_str("sha256:");
    for byte in bytes.as_ref() {
        write!(hex, "{byte:02x}").ok();
    }
    hex
}

fn generate_token() -> String {
    let random: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(TOKEN_RANDOM_LEN)
        .map(char::from)
        .collect();
    format!("{TOKEN_PREFIX}{random}")
}

fn validate_name(name: &str) -> Result<(), A2aKeyError> {
    let well_formed = name.len() <= MAX_NAME_LEN
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if well_formed {
        Ok(())
    } else {
        Err(A2aKeyError::Invalid(format!(
            "caller key name '{name}' must start with a lowercase letter and contain only \
             lowercase letters, digits, and underscores (at most {MAX_NAME_LEN} characters)"
        )))
    }
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "signature matches the unix variant, which can fail"
)]
fn set_owner_only(path: &Path) -> std::io::Result<()> {
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "CURRENT_USER".to_string());
    let grant_arg = format!("{username}:(F)");
    let status = std::process::Command::new("icacls")
        .arg(path.as_os_str())
        .args(["/inheritance:r", "/grant:r", &grant_arg])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => tracing::warn!(
            path = %path.display(),
            exit_code = ?s.code(),
            "icacls failed to restrict A2A caller keys file permissions"
        ),
        Err(e) => tracing::warn!(
            path = %path.display(),
            error = %e,
            "could not run icacls to restrict A2A caller keys file permissions"
        ),
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn set_owner_only(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_verify_roundtrips_and_never_stores_the_token() {
        let mut store = A2aKeyStore::default();
        let token = store.create("laptop", Some("my other instance")).unwrap();
        assert!(token.starts_with(TOKEN_PREFIX));
        assert_eq!(token.len(), TOKEN_PREFIX.len() + TOKEN_RANDOM_LEN);

        assert_eq!(store.verify(&token), Some("laptop".to_string()));
        assert_eq!(
            store.verify("rsdm_a2a_wrongwrongwrongwrongwrongwrongwr"),
            None
        );

        let dir = tempfile::tempdir().unwrap();
        store.save(dir.path()).unwrap();
        let raw = std::fs::read_to_string(dir.path().join(KEYS_FILE)).unwrap();
        assert!(
            !raw.contains(&token),
            "the store file must never contain the plaintext token"
        );
        assert!(raw.contains("sha256:"), "the store file should hold a hash");
    }

    #[test]
    fn save_load_roundtrip_keeps_metadata_and_verification() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = A2aKeyStore::default();
        let token = store.create("beta", Some("second instance")).unwrap();
        store.save(dir.path()).unwrap();

        let loaded = A2aKeyStore::load(dir.path()).unwrap();
        assert_eq!(loaded.verify(&token), Some("beta".to_string()));
        let list = loaded.list();
        let [only] = list.as_slice() else {
            panic!("expected exactly one key, got {list:?}");
        };
        assert_eq!(only.name, "beta");
        assert_eq!(only.description, "second instance");
    }

    #[test]
    fn missing_file_is_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = A2aKeyStore::load(dir.path()).unwrap();
        let actual = &store.list();
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let mut store = A2aKeyStore::default();
        store.create("beta", None).unwrap();
        assert_eq!(
            store.create("beta", None),
            Err(A2aKeyError::AlreadyExists("beta".to_string()))
        );
    }

    #[test]
    fn revoke_unknown_key_is_not_found() {
        let mut store = A2aKeyStore::default();
        assert_eq!(
            store.revoke("nope"),
            Err(A2aKeyError::NotFound("nope".to_string()))
        );
    }

    #[test]
    fn revoke_removes_the_key() {
        let mut store = A2aKeyStore::default();
        let token = store.create("beta", None).unwrap();
        store.revoke("beta").unwrap();
        assert_eq!(store.verify(&token), None);
    }

    #[test]
    fn name_validation() {
        for good in ["a", "laptop", "second_instance", "x1"] {
            assert!(validate_name(good).is_ok(), "'{good}' should be accepted");
        }
        for bad in ["", "Laptop", "1x", "_x", "has-dash", "has space"] {
            assert!(validate_name(bad).is_err(), "'{bad}' should be rejected");
        }
        assert!(validate_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut store = A2aKeyStore::default();
        store.create("beta", None).unwrap();
        store.save(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join(KEYS_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn debug_output_never_contains_hashes_or_tokens() {
        let mut store = A2aKeyStore::default();
        let token = store.create("beta", None).unwrap();
        let debug = format!("{store:?}");
        assert!(debug.contains("beta"));
        assert!(!debug.contains(&token));
    }
}
