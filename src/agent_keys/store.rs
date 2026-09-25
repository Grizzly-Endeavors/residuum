//! Encrypted on-disk store of agent keys.
//!
//! Lives in `agent-keys.toml.enc` with its own machine key in `agent-keys.key`,
//! both in the residuum config directory and both separate from the system
//! secret store (`secrets.toml.enc`) so agent-facing code can never reach a
//! provider key. Encryption is the same AES-256-GCM-SIV scheme as
//! [`crate::config::SecretStore`].
//!
//! The plaintext format before encryption:
//! ```toml
//! [keys.github_token]
//! value = "ghp_..."
//! description = "Fine-grained PAT for Grizzly-Endeavors"
//! created_by = "user"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::AgentKeyError;
use crate::config::secrets::{decrypt, encrypt, load_key, load_or_create_key};

/// Encrypted store file name within the config directory.
pub(crate) const ENCRYPTED_FILE: &str = "agent-keys.toml.enc";

/// Machine key file name within the config directory.
pub(crate) const KEY_FILE: &str = "agent-keys.key";

/// Lock file serializing writes across processes.
pub(crate) const LOCK_FILE: &str = "agent-keys.lock";

/// Longest accepted key name.
const MAX_NAME_LEN: usize = 64;

/// Key value length below which redaction by substring match becomes
/// unreliable (a short value is more likely to also appear as ordinary,
/// unrelated output). Not a hard minimum: a value under this length is still
/// stored, with a warning naming the risk (see [`short_value_warning`]).
const SHORT_VALUE_WARNING_LEN: usize = 8;

/// Environment variables a key name must not map onto: overriding them would
/// break or hijack the spawned shell rather than hand it a credential.
const RESERVED_ENV_VARS: &[&str] = &[
    "PATH", "HOME", "USER", "SHELL", "PWD", "TMPDIR", "IFS", "ENV", "BASH_ENV", "LANG",
];

/// Environment variable prefixes a key name must not map onto (dynamic
/// loader controls on Linux and macOS).
const RESERVED_ENV_PREFIXES: &[&str] = &["LD_", "DYLD_"];

/// Who created a key, which decides who may overwrite or delete it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyCreator {
    /// Stored by the user through the CLI or web UI.
    User,
    /// Minted by the agent with `exec`'s `store_output_as`.
    Agent,
}

impl KeyCreator {
    /// Lowercase label, as stored and as shown in listings.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct KeyEntry {
    value: String,
    #[serde(default)]
    description: String,
    created_by: KeyCreator,
}

#[derive(Serialize, Deserialize, Default)]
struct KeysFile {
    #[serde(default)]
    keys: BTreeMap<String, KeyEntry>,
}

/// Everything about a key except its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentKeyInfo {
    pub name: String,
    pub env_var: String,
    pub description: String,
    pub created_by: KeyCreator,
}

/// Decrypted agent keys, keyed by name.
#[derive(Default, Clone)]
pub struct AgentKeyStore {
    keys: BTreeMap<String, KeyEntry>,
}

impl std::fmt::Debug for AgentKeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentKeyStore")
            .field("names", &self.keys.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl AgentKeyStore {
    /// Load from the encrypted file in `config_dir`. A missing file is an
    /// empty store.
    ///
    /// # Errors
    /// Returns `AgentKeyError::Storage` if the key or store file can't be
    /// read, or decryption or parsing fails.
    pub fn load(config_dir: &Path) -> Result<Self, AgentKeyError> {
        let enc_path = config_dir.join(ENCRYPTED_FILE);
        if !enc_path.exists() {
            return Ok(Self::default());
        }

        let key = load_key(&config_dir.join(KEY_FILE))
            .map_err(|e| AgentKeyError::Storage(e.to_string()))?;
        let ciphertext = std::fs::read(&enc_path).map_err(|e| {
            AgentKeyError::Storage(format!(
                "failed to read agent keys file at {}: {e}",
                enc_path.display()
            ))
        })?;
        let plaintext = decrypt(&ciphertext, &key).map_err(|e| {
            AgentKeyError::Storage(format!(
                "failed to decrypt agent keys at {}: {e}",
                enc_path.display()
            ))
        })?;
        let file: KeysFile = toml::from_str(&plaintext).map_err(|e| {
            AgentKeyError::Storage(format!(
                "failed to parse decrypted agent keys at {}: {e}",
                enc_path.display()
            ))
        })?;

        Ok(Self { keys: file.keys })
    }

    /// Encrypt and write the store to `config_dir`, creating the machine key
    /// on first use. Writes to a temporary file and renames it into place so
    /// a crash mid-write can't destroy every stored key.
    ///
    /// # Errors
    /// Returns `AgentKeyError::Storage` if encryption or any file operation
    /// fails.
    pub fn save(&self, config_dir: &Path) -> Result<(), AgentKeyError> {
        let key = load_or_create_key(&config_dir.join(KEY_FILE))
            .map_err(|e| AgentKeyError::Storage(e.to_string()))?;
        let plaintext = toml::to_string(&KeysFile {
            keys: self.keys.clone(),
        })
        .map_err(|e| AgentKeyError::Storage(format!("failed to serialize agent keys: {e}")))?;
        let ciphertext = encrypt(&plaintext, &key)
            .map_err(|e| AgentKeyError::Storage(format!("failed to encrypt agent keys: {e}")))?;

        let enc_path = config_dir.join(ENCRYPTED_FILE);
        let tmp_path: PathBuf = config_dir.join(format!("{ENCRYPTED_FILE}.tmp"));
        std::fs::write(&tmp_path, &ciphertext).map_err(|e| {
            AgentKeyError::Storage(format!(
                "failed to write agent keys file at {}: {e}",
                tmp_path.display()
            ))
        })?;
        std::fs::rename(&tmp_path, &enc_path).map_err(|e| {
            AgentKeyError::Storage(format!(
                "failed to move agent keys file into place at {}: {e}",
                enc_path.display()
            ))
        })
    }

    /// The value of a key, if it exists.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&str> {
        self.keys.get(name).map(|e| e.value.as_str())
    }

    /// Who created a key, if it exists.
    #[must_use]
    pub fn creator(&self, name: &str) -> Option<KeyCreator> {
        self.keys.get(name).map(|e| e.created_by)
    }

    /// Every key's name, environment variable, description, and creator —
    /// never values. Sorted by name.
    #[must_use]
    pub fn list(&self) -> Vec<AgentKeyInfo> {
        self.keys
            .iter()
            .map(|(name, entry)| AgentKeyInfo {
                name: name.clone(),
                env_var: env_var_for(name),
                description: entry.description.clone(),
                created_by: entry.created_by,
            })
            .collect()
    }

    /// Sorted key names.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.keys.keys().map(String::as_str).collect()
    }

    /// `(name, value)` for every key.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.keys
            .iter()
            .map(|(name, entry)| (name.as_str(), entry.value.as_str()))
    }

    /// Insert or replace a key in memory. `description: None` keeps an
    /// existing key's description. The caller persists with [`save`](Self::save).
    ///
    /// Returns a warning (the key is still stored either way) when `value`
    /// is short enough that redaction by substring match becomes unreliable.
    ///
    /// # Errors
    /// Returns `AgentKeyError::Invalid` for a bad name or value, and
    /// `AgentKeyError::OwnedByUser` when the agent tries to replace a key
    /// the user created.
    pub fn set(
        &mut self,
        name: &str,
        value: &str,
        description: Option<&str>,
        creator: KeyCreator,
    ) -> Result<Option<String>, AgentKeyError> {
        validate_name(name)?;
        validate_value(value)?;
        let existing = self.keys.get(name);
        if creator == KeyCreator::Agent
            && existing.is_some_and(|e| e.created_by == KeyCreator::User)
        {
            return Err(AgentKeyError::OwnedByUser(name.to_string()));
        }
        let description = description.map_or_else(
            || existing.map(|e| e.description.clone()).unwrap_or_default(),
            |d| d.trim().to_string(),
        );
        let warning = short_value_warning(value);
        self.keys.insert(
            name.to_string(),
            KeyEntry {
                value: value.to_string(),
                description,
                created_by: creator,
            },
        );
        Ok(warning)
    }

    /// Remove a key in memory. The caller persists with [`save`](Self::save).
    ///
    /// # Errors
    /// Returns `AgentKeyError::NotFound` for an unknown name and
    /// `AgentKeyError::OwnedByUser` when the agent tries to delete a key the
    /// user created.
    pub fn delete(&mut self, name: &str, requester: KeyCreator) -> Result<(), AgentKeyError> {
        match self.keys.get(name) {
            None => Err(AgentKeyError::NotFound(name.to_string())),
            Some(e) if requester == KeyCreator::Agent && e.created_by == KeyCreator::User => {
                Err(AgentKeyError::OwnedByUser(name.to_string()))
            }
            Some(_) => {
                self.keys.remove(name);
                Ok(())
            }
        }
    }
}

/// The environment variable a key is exposed as: its name uppercased.
#[must_use]
pub fn env_var_for(name: &str) -> String {
    name.to_ascii_uppercase()
}

/// Check a key name: `[a-z][a-z0-9_]*`, at most 64 characters, and not
/// mapping onto a reserved environment variable.
///
/// # Errors
/// Returns `AgentKeyError::Invalid` describing what is wrong.
pub fn validate_name(name: &str) -> Result<(), AgentKeyError> {
    let well_formed = name.len() <= MAX_NAME_LEN
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !well_formed {
        return Err(AgentKeyError::Invalid(format!(
            "key name '{name}' must start with a lowercase letter and contain only lowercase \
             letters, digits, and underscores (at most {MAX_NAME_LEN} characters)"
        )));
    }
    let env_var = env_var_for(name);
    if RESERVED_ENV_VARS.contains(&env_var.as_str())
        || RESERVED_ENV_PREFIXES.iter().any(|p| env_var.starts_with(p))
    {
        return Err(AgentKeyError::Invalid(format!(
            "key name '{name}' would override the ${env_var} environment variable; choose another name"
        )));
    }
    Ok(())
}

/// Check a key value: no NUL byte. There is no minimum length — see
/// [`short_value_warning`] for the (non-blocking) redaction-risk warning on
/// a short one.
///
/// # Errors
/// Returns `AgentKeyError::Invalid` describing what is wrong.
pub fn validate_value(value: &str) -> Result<(), AgentKeyError> {
    if value.contains('\0') {
        return Err(AgentKeyError::Invalid(
            "key value must not contain a NUL byte".to_string(),
        ));
    }
    Ok(())
}

/// A warning that `value` is short enough that redacting it from tool/log
/// output by substring match is unreliable — it's more likely to also occur
/// as ordinary, unrelated output — or `None` when it's long enough not to
/// matter. The value is stored either way; this is advisory only.
#[must_use]
pub fn short_value_warning(value: &str) -> Option<String> {
    let len = value.chars().count();
    (len < SHORT_VALUE_WARNING_LEN).then(|| {
        format!(
            "this key's value is only {len} character(s); values under \
             {SHORT_VALUE_WARNING_LEN} can't be redacted from output reliably, since a short \
             string is more likely to also appear as ordinary, unrelated text"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_roundtrip_keeps_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = AgentKeyStore::default();
        store
            .set(
                "github_token",
                "ghp_abcdefgh1234",
                Some("repo access"),
                KeyCreator::User,
            )
            .unwrap();
        store.save(dir.path()).unwrap();

        let loaded = AgentKeyStore::load(dir.path()).unwrap();
        assert_eq!(loaded.value("github_token"), Some("ghp_abcdefgh1234"));
        assert_eq!(
            loaded.list(),
            vec![AgentKeyInfo {
                name: "github_token".to_string(),
                env_var: "GITHUB_TOKEN".to_string(),
                description: "repo access".to_string(),
                created_by: KeyCreator::User,
            }]
        );
    }

    #[test]
    fn store_uses_its_own_key_file_not_the_secret_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = AgentKeyStore::default();
        store
            .set("api", "value-12345678", None, KeyCreator::User)
            .unwrap();
        store.save(dir.path()).unwrap();

        assert!(dir.path().join(KEY_FILE).exists());
        assert!(dir.path().join(ENCRYPTED_FILE).exists());
        assert!(
            !dir.path().join("secrets.key").exists(),
            "agent keys must not create or use the system secret key"
        );
        let raw = std::fs::read(dir.path().join(ENCRYPTED_FILE)).unwrap();
        assert!(
            !String::from_utf8_lossy(&raw).contains("value-12345678"),
            "store file must be encrypted"
        );
    }

    #[test]
    fn missing_file_is_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentKeyStore::load(dir.path()).unwrap();
        assert!(store.names().is_empty());
    }

    #[test]
    fn agent_cannot_overwrite_or_delete_user_keys() {
        let mut store = AgentKeyStore::default();
        store
            .set("shared", "user-value-123", None, KeyCreator::User)
            .unwrap();

        let overwrite = store.set("shared", "agent-value-123", None, KeyCreator::Agent);
        assert_eq!(
            overwrite,
            Err(AgentKeyError::OwnedByUser("shared".to_string()))
        );
        assert_eq!(store.value("shared"), Some("user-value-123"));

        let delete = store.delete("shared", KeyCreator::Agent);
        assert_eq!(
            delete,
            Err(AgentKeyError::OwnedByUser("shared".to_string()))
        );
        assert!(store.value("shared").is_some());
    }

    #[test]
    fn agent_can_replace_and_delete_its_own_keys() {
        let mut store = AgentKeyStore::default();
        store
            .set("minted", "first-value-1", Some("desc"), KeyCreator::Agent)
            .unwrap();
        store
            .set("minted", "second-value-2", None, KeyCreator::Agent)
            .unwrap();
        assert_eq!(store.value("minted"), Some("second-value-2"));
        assert_eq!(
            store.list().first().map(|k| k.description.as_str()),
            Some("desc"),
            "omitted description keeps the existing one"
        );
        store.delete("minted", KeyCreator::Agent).unwrap();
        assert!(store.value("minted").is_none());
    }

    #[test]
    fn user_can_overwrite_agent_keys() {
        let mut store = AgentKeyStore::default();
        store
            .set("minted", "agent-value-1", None, KeyCreator::Agent)
            .unwrap();
        store
            .set("minted", "user-value-12", None, KeyCreator::User)
            .unwrap();
        assert_eq!(store.creator("minted"), Some(KeyCreator::User));
    }

    #[test]
    fn delete_unknown_key_is_not_found() {
        let mut store = AgentKeyStore::default();
        assert_eq!(
            store.delete("nope", KeyCreator::User),
            Err(AgentKeyError::NotFound("nope".to_string()))
        );
    }

    #[test]
    fn name_validation() {
        for good in ["a", "github_token", "k8s_api", "x_1"] {
            assert!(validate_name(good).is_ok(), "'{good}' should be accepted");
        }
        for bad in [
            "",
            "Github",
            "1token",
            "_token",
            "has-dash",
            "has space",
            "path",
            "home",
            "ld_preload",
            "dyld_insert_libraries",
        ] {
            assert!(validate_name(bad).is_err(), "'{bad}' should be rejected");
        }
        assert!(validate_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn value_validation() {
        assert!(validate_value("12345678").is_ok());
        assert!(
            validate_value("1234567").is_ok(),
            "there is no length minimum; a short value is stored, just warned about"
        );
        assert!(validate_value("abcd\0efgh").is_err(), "NUL rejected");
    }

    #[test]
    fn short_value_warning_only_below_the_threshold() {
        assert!(short_value_warning("12345678").is_none());
        let warning = short_value_warning("1234567").expect("a short value should warn");
        assert!(warning.contains('7'), "warning should name the length");
        assert!(warning.contains("redact"));
    }

    #[test]
    fn debug_output_never_contains_values() {
        let mut store = AgentKeyStore::default();
        store
            .set("tok", "super-secret-value", None, KeyCreator::User)
            .unwrap();
        let debug = format!("{store:?}");
        assert!(debug.contains("tok"));
        assert!(!debug.contains("super-secret-value"));
    }
}
