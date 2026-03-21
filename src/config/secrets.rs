//! Encrypted secret storage using AES-256-GCM-SIV.
//!
//! Secrets are stored in `secrets.toml.enc` (encrypted TOML) alongside a machine
//! key in `secrets.key` (mode 0600, 256-bit random). Both files live in the
//! residuum config directory (typically `~/.residuum/`).
//!
//! The plaintext format before encryption:
//! ```toml
//! [secrets]
//! anthropic_key = "sk-ant-..."
//! openai_key = "sk-..."
//! ```

use std::collections::HashMap;
use std::path::Path;

use aes_gcm_siv::aead::Aead;
use aes_gcm_siv::{Aes256GcmSiv, KeyInit, Nonce};

use crate::error::FatalError;

/// File names within the config directory.
const KEY_FILE: &str = "secrets.key";
const ENCRYPTED_FILE: &str = "secrets.toml.enc";

/// Nonce size for AES-256-GCM-SIV (96 bits / 12 bytes).
const NONCE_SIZE: usize = 12;

/// In-memory representation of the decrypted secret store.
pub struct SecretStore {
    secrets: HashMap<String, String>,
}

impl SecretStore {
    /// Load from the encrypted file. Returns an empty store if the file doesn't exist.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if the key or encrypted file cannot be
    /// read, or if decryption or TOML parsing fails.
    pub fn load(config_dir: &Path) -> Result<Self, FatalError> {
        let enc_path = config_dir.join(ENCRYPTED_FILE);
        if !enc_path.exists() {
            return Ok(Self {
                secrets: HashMap::new(),
            });
        }

        let key = load_key(config_dir)?;
        let ciphertext = std::fs::read(&enc_path).map_err(|e| {
            FatalError::Config(format!(
                "failed to read secrets file at {}: {e}",
                enc_path.display()
            ))
        })?;

        let plaintext = decrypt(&ciphertext, &key)?;
        let table = parse_secrets_toml(&plaintext)?;

        let store = Self { secrets: table };
        tracing::debug!(count = store.secrets.len(), "secrets loaded");
        Ok(store)
    }

    /// Get a secret by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.secrets.get(name).map(String::as_str)
    }

    /// Set a secret and persist the entire store to disk.
    ///
    /// Creates the key file if it doesn't exist yet.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if the store cannot be saved.
    pub fn set(&mut self, name: &str, value: &str, config_dir: &Path) -> Result<(), FatalError> {
        self.secrets.insert(name.to_owned(), value.to_owned());
        self.save(config_dir)
    }

    /// Delete a secret and persist. No-op if the secret doesn't exist.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if the store cannot be saved.
    pub fn delete(&mut self, name: &str, config_dir: &Path) -> Result<(), FatalError> {
        self.secrets.remove(name);
        self.save(config_dir)
    }

    /// List all secret names (not values).
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.secrets.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Serialize and encrypt the store to disk.
    fn save(&self, config_dir: &Path) -> Result<(), FatalError> {
        let key = load_or_create_key(config_dir)?;
        let plaintext = serialize_secrets_toml(&self.secrets);
        let ciphertext = encrypt(&plaintext, &key)?;

        let enc_path = config_dir.join(ENCRYPTED_FILE);
        std::fs::write(&enc_path, &ciphertext).map_err(|e| {
            FatalError::Config(format!(
                "failed to write secrets file at {}: {e}",
                enc_path.display()
            ))
        })
    }
}

/// Load an existing key file, or return an error if it doesn't exist.
fn load_key(config_dir: &Path) -> Result<[u8; 32], FatalError> {
    let key_path = config_dir.join(KEY_FILE);
    let bytes = std::fs::read(&key_path).map_err(|e| {
        FatalError::Config(format!(
            "failed to read secret key at {}: {e}",
            key_path.display()
        ))
    })?;

    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|err| {
        FatalError::Config(format!(
            "secret key at {} has invalid length (expected 32 bytes, got {}): {err}",
            key_path.display(),
            bytes.len()
        ))
    })
}

/// Load an existing key or generate a new one on first use.
fn load_or_create_key(config_dir: &Path) -> Result<[u8; 32], FatalError> {
    let key_path = config_dir.join(KEY_FILE);
    if key_path.exists() {
        return load_key(config_dir);
    }

    // Generate a new random key
    let key: [u8; 32] = rand::random();

    // Ensure config dir exists
    std::fs::create_dir_all(config_dir).map_err(|e| {
        FatalError::Config(format!(
            "failed to create config directory {}: {e}",
            config_dir.display()
        ))
    })?;

    std::fs::write(&key_path, key).map_err(|e| {
        FatalError::Config(format!(
            "failed to write secret key at {}: {e}",
            key_path.display()
        ))
    })?;

    // Set permissions to 0o600 (owner-only read/write)
    set_file_mode_600(&key_path)?;

    tracing::info!(path = %key_path.display(), "generated new encryption key");

    Ok(key)
}

/// Set file permissions to 0600 (Unix only).
#[cfg(unix)]
fn set_file_mode_600(path: &Path) -> Result<(), FatalError> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms).map_err(|e| {
        FatalError::Config(format!(
            "failed to set permissions on {}: {e}",
            path.display()
        ))
    })
}

#[cfg(not(unix))]
fn set_file_mode_600(_path: &Path) -> Result<(), FatalError> {
    // No-op on non-Unix platforms
    Ok(())
}

/// Encrypt plaintext using AES-256-GCM-SIV with a random nonce.
///
/// Output format: nonce (12 bytes) || ciphertext + auth tag.
fn encrypt(plaintext: &str, key: &[u8; 32]) -> Result<Vec<u8>, FatalError> {
    let cipher = Aes256GcmSiv::new(key.into());
    let nonce_bytes: [u8; NONCE_SIZE] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| FatalError::Config(format!("failed to encrypt secrets: {e}")))?;

    let mut output = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypt ciphertext produced by [`encrypt`].
fn decrypt(data: &[u8], key: &[u8; 32]) -> Result<String, FatalError> {
    if data.len() < NONCE_SIZE {
        return Err(FatalError::Config(
            "encrypted secrets file is too short (missing nonce)".to_string(),
        ));
    }

    let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
    let cipher = Aes256GcmSiv::new(key.into());
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| FatalError::Config(format!("failed to decrypt secrets: {e}")))?;

    String::from_utf8(plaintext)
        .map_err(|e| FatalError::Config(format!("decrypted secrets contain invalid UTF-8: {e}")))
}

/// Parse the decrypted TOML into a flat key→value map.
fn parse_secrets_toml(toml_str: &str) -> Result<HashMap<String, String>, FatalError> {
    #[derive(serde::Deserialize)]
    struct SecretsFile {
        #[serde(default)]
        secrets: HashMap<String, String>,
    }

    let file: SecretsFile = toml::from_str(toml_str)
        .map_err(|e| FatalError::Config(format!("failed to parse decrypted secrets TOML: {e}")))?;

    Ok(file.secrets)
}

/// Serialize secrets to TOML format.
fn serialize_secrets_toml(secrets: &HashMap<String, String>) -> String {
    let mut lines = Vec::with_capacity(secrets.len() + 1);
    lines.push("[secrets]".to_string());

    let mut keys: Vec<&String> = secrets.keys().collect();
    keys.sort_unstable();

    for key in keys {
        let escaped = secrets[key].replace('\\', "\\\\").replace('"', "\\\"");
        lines.push(format!("{key} = \"{escaped}\""));
    }

    lines.join("\n")
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn create_key_and_encrypt_decrypt_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let key = load_or_create_key(dir.path()).unwrap();

        let plaintext = "[secrets]\ntest_key = \"my-secret-value\"";
        let ciphertext = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&ciphertext, &key).unwrap();

        assert_eq!(decrypted, plaintext, "roundtrip should preserve content");
    }

    #[test]
    fn load_empty_returns_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::load(dir.path()).unwrap();

        assert!(
            store.secrets.is_empty(),
            "missing file should yield empty store"
        );
    }

    #[test]
    fn set_get_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SecretStore::load(dir.path()).unwrap();

        store.set("my_key", "my_value", dir.path()).unwrap();
        assert_eq!(
            store.get("my_key"),
            Some("my_value"),
            "should get back what was set"
        );

        // Reload from disk to verify persistence
        let reloaded = SecretStore::load(dir.path()).unwrap();
        assert_eq!(
            reloaded.get("my_key"),
            Some("my_value"),
            "should persist across loads"
        );

        // Delete
        let mut store2 = reloaded;
        store2.delete("my_key", dir.path()).unwrap();
        assert!(
            store2.get("my_key").is_none(),
            "should be gone after delete"
        );

        // Verify deletion persisted
        let reloaded2 = SecretStore::load(dir.path()).unwrap();
        assert!(
            reloaded2.get("my_key").is_none(),
            "deletion should persist across loads"
        );
    }

    #[test]
    fn names_returns_keys_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SecretStore::load(dir.path()).unwrap();

        store.set("beta_key", "val1", dir.path()).unwrap();
        store.set("alpha_key", "val2", dir.path()).unwrap();

        let names = store.names();
        assert_eq!(
            names,
            vec!["alpha_key", "beta_key"],
            "names should be sorted"
        );
    }
}
