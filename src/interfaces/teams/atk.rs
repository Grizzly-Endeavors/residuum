//! Microsoft 365 Agents Toolkit (ATK) credential decryption and project import.
//!
//! Provides routines to decrypt the encrypted bot password produced by ATK's
//! `writeToEnvironmentFile` driver (using Cryptr / AES-256-GCM), parse ATK project
//! outputs, store the decrypted secret in Residuum's [`SecretStore`], and configure
//! the agent's `[teams]` section via the checkpointed config path.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use aes::Aes256;
use aes::cipher::{BlockEncrypt, KeyInit};
use ring::pbkdf2;

use crate::checkpoints::{CheckpointContext, CheckpointEngine, CheckpointTrigger, RepoKind};
use crate::config::paths::{agent_dir, hub_dir};
use crate::config::secrets::SecretStore;
use crate::config::{Config, HubConfig};
use crate::util::FatalError;

/// Pinned CLI version constant for Microsoft 365 Agents Toolkit.
pub const ATK_CLI_VERSION: &str = "1.1.17";

/// Pinned npm package name for Microsoft 365 Agents Toolkit CLI.
pub const ATK_PACKAGE_NAME: &str = "@microsoft/m365agentstoolkit-cli";

/// Default global key used by ATK `LocalCrypto.fixedCryptr`.
pub const ATK_DEFAULT_GLOBAL_KEY: &str = "teamsfx_global_key";

/// Prefix prepended to hex-encoded encrypted secrets by ATK `LocalCrypto`.
pub const CRYPTO_PREFIX: &str = "crypto_";

/// Error returned when ATK secret decryption fails.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AtkDecryptError {
    #[error("encrypted secret has invalid format: {0}")]
    InvalidFormat(String),
    #[error(
        "decryption failed: cipher text is broken or key mismatch. You can create a new client secret manually in Microsoft Teams Developer Portal (https://dev.teams.microsoft.com/bots) and store it with `residuum secret set teams`."
    )]
    DecryptionFailed(String),
}

/// Decrypt an ATK secret encrypted with Cryptr / `LocalCrypto` (AES-256-GCM).
///
/// Follows `@microsoft/teamsfx-core` `LocalCrypto.decrypt`:
/// 1. If `ciphertext` does not start with `"crypto_"`, returns `ciphertext` as-is (legacy raw plaintext).
/// 2. Strips `"crypto_"` and decodes lowercase hex.
/// 3. Extracts salt (64 bytes), IV (16 bytes), tag (16 bytes), and ciphertext (remainder).
/// 4. Tries decryption using `"teamsfx_global_key"` via PBKDF2-HMAC-SHA512 (100,000 iterations, 32-byte key).
/// 5. If that fails and `project_id` is supplied, tries fallback secret `<project_id>_teamsfx`.
/// 6. If both fail, returns [`AtkDecryptError::DecryptionFailed`].
///
/// # Errors
///
/// Returns [`AtkDecryptError::InvalidFormat`] if the payload is not valid hex or too short.
/// Returns [`AtkDecryptError::DecryptionFailed`] if decryption fails with available keys.
pub fn decrypt_atk_secret(
    ciphertext: &str,
    project_id: Option<&str>,
) -> Result<String, AtkDecryptError> {
    let Some(hex_part) = ciphertext.strip_prefix(CRYPTO_PREFIX) else {
        return Ok(ciphertext.to_string());
    };

    let raw = hex::decode(hex_part)
        .map_err(|e| AtkDecryptError::InvalidFormat(format!("invalid hex: {e}")))?;

    // Minimum length: 64 (salt) + 16 (IV) + 16 (tag) = 96 bytes
    let (salt, rest) = raw.split_at_checked(64).ok_or_else(|| {
        AtkDecryptError::InvalidFormat(format!(
            "payload length {} is less than minimum header size of 96 bytes",
            raw.len()
        ))
    })?;
    let (iv, rest) = rest.split_at_checked(16).ok_or_else(|| {
        AtkDecryptError::InvalidFormat(format!(
            "payload length {} is less than minimum header size of 96 bytes",
            raw.len()
        ))
    })?;
    let (tag, enc) = rest.split_at_checked(16).ok_or_else(|| {
        AtkDecryptError::InvalidFormat(format!(
            "payload length {} is less than minimum header size of 96 bytes",
            raw.len()
        ))
    })?;

    // Attempt 1: primary fixed global key
    if let Ok(plain) = decrypt_gcm_with_secret(ATK_DEFAULT_GLOBAL_KEY, salt, iv, tag, enc) {
        return Ok(plain);
    }

    // Attempt 2: fallback project key
    if let Some(pid) = project_id {
        let fallback_secret = format!("{pid}_teamsfx");
        if let Ok(plain) = decrypt_gcm_with_secret(&fallback_secret, salt, iv, tag, enc) {
            return Ok(plain);
        }
    }

    Err(AtkDecryptError::DecryptionFailed(
        "failed to decrypt with both global key and project fallback".to_string(),
    ))
}

/// Decrypt AES-256-GCM using Cryptr's 16-byte IV layout and PBKDF2 derivation.
#[expect(
    clippy::indexing_slicing,
    reason = "cryptographic block and counter indexing with fixed-size buffers"
)]
fn decrypt_gcm_with_secret(
    secret: &str,
    salt: &[u8],
    iv: &[u8],
    tag: &[u8],
    enc: &[u8],
) -> Result<String, ()> {
    // 1. Derive 32-byte key using PBKDF2-HMAC-SHA512 (100,000 iterations)
    let mut key = [0_u8; 32];
    let iterations = std::num::NonZeroU32::new(100_000).ok_or(())?;
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA512,
        iterations,
        salt,
        secret.as_bytes(),
        &mut key,
    );

    // 2. AES-256 cipher
    let cipher = Aes256::new_from_slice(&key).map_err(|_err| ())?;

    // 3. Compute H = AES(key, 0^16)
    let mut h = [0_u8; 16];
    cipher.encrypt_block((&mut h).into());

    // 4. Compute J0 for 16-byte IV (NIST SP 800-38D Section 7.1)
    // Block 1: IV (16 bytes)
    // Block 2: 0^8 || [128]_64 (len(IV) in bits as 64-bit big endian integer)
    let mut iv_block2 = [0_u8; 16];
    iv_block2[8..16].copy_from_slice(&128_u64.to_be_bytes());
    let mut j0_data = [0_u8; 32];
    j0_data[0..16].copy_from_slice(iv);
    j0_data[16..32].copy_from_slice(&iv_block2);
    let j0 = ghash(&h, &j0_data);

    // 5. Decrypt CTR
    let mut decrypted = Vec::with_capacity(enc.len());
    let mut ctr = j0;
    let mut offset = 0;
    while offset < enc.len() {
        inc32(&mut ctr);
        let mut ks = ctr;
        cipher.encrypt_block((&mut ks).into());
        let chunk_len = (enc.len() - offset).min(16);
        for i in 0..chunk_len {
            decrypted.push(enc[offset + i] ^ ks[i]);
        }
        offset += chunk_len;
    }

    // 6. Verify Tag
    // GHASH data: enc padded to 16 bytes || [0]_64 || [len(enc)*8]_64
    let pad_len = (16 - (enc.len() % 16)) % 16;
    let total_ghash_len = enc.len() + pad_len + 16;
    let mut ghash_buf = vec![0_u8; total_ghash_len];
    ghash_buf[..enc.len()].copy_from_slice(enc);
    let bit_len = (enc.len() as u64) * 8;
    ghash_buf[total_ghash_len - 8..total_ghash_len].copy_from_slice(&bit_len.to_be_bytes());

    let s = ghash(&h, &ghash_buf);
    let mut tag_mask = j0;
    cipher.encrypt_block((&mut tag_mask).into());
    let mut expected_tag = [0_u8; 16];
    for i in 0..16 {
        expected_tag[i] = s[i] ^ tag_mask[i];
    }

    if subtle_eq(tag, &expected_tag) {
        String::from_utf8(decrypted).map_err(|_err| ())
    } else {
        Err(())
    }
}

/// GF(2^128) multiplication per NIST SP 800-38D Section 6.3.
#[expect(
    clippy::indexing_slicing,
    reason = "cryptographic bit and byte indexing in GF(2^128) arithmetic"
)]
fn gf_mul(x: &[u8; 16], y: &[u8; 16]) -> [u8; 16] {
    let mut v = *y;
    let mut z = [0_u8; 16];
    for i in 0..128 {
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8);
        let bit = (x[byte_idx] >> bit_idx) & 1;
        if bit == 1 {
            for j in 0..16 {
                z[j] ^= v[j];
            }
        }
        let lsb = v[15] & 1;
        for j in (1..16).rev() {
            v[j] = (v[j] >> 1) | ((v[j - 1] & 1) << 7);
        }
        v[0] >>= 1;
        if lsb == 1 {
            v[0] ^= 0xe1;
        }
    }
    z
}

/// GHASH function per NIST SP 800-38D Section 6.4.
#[expect(
    clippy::indexing_slicing,
    reason = "GHASH block copying with 16-byte chunks"
)]
fn ghash(h: &[u8; 16], data: &[u8]) -> [u8; 16] {
    let mut y = [0_u8; 16];
    for chunk in data.chunks(16) {
        let mut block = [0_u8; 16];
        block[..chunk.len()].copy_from_slice(chunk);
        for i in 0..16 {
            y[i] ^= block[i];
        }
        y = gf_mul(&y, h);
    }
    y
}

/// Increment 32-bit counter in the rightmost 4 bytes of a 16-byte block.
fn inc32(block: &mut [u8; 16]) {
    let mut val = u32::from_be_bytes([block[12], block[13], block[14], block[15]]);
    val = val.wrapping_add(1);
    block[12..16].copy_from_slice(&val.to_be_bytes());
}

/// Constant-time comparison for authentication tags.
fn subtle_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Parsed output variables from an ATK project directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtkProjectEnv {
    /// Bot Application (Client) ID.
    pub bot_id: String,
    /// Microsoft Entra Directory (Tenant) ID.
    pub teams_app_tenant_id: String,
    /// Teams App Definition ID (optional).
    pub teams_app_id: Option<String>,
    /// Decrypted bot password / client secret.
    pub bot_password: String,
}

/// Result of importing an ATK project into Residuum.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportAtkResult {
    /// Bot Application (Client) ID.
    pub bot_id: String,
    /// Microsoft Entra Directory (Tenant) ID.
    pub tenant_id: String,
    /// Teams App Definition ID if present.
    pub teams_app_id: Option<String>,
}

/// Parse a simple `.env` key=value file, ignoring comments and whitespace.
fn parse_env_file(path: &Path) -> Result<HashMap<String, String>, FatalError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        FatalError::Config(format!(
            "failed to read env file at {}: {e}",
            path.display()
        ))
    })?;

    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_string();
            let mut val = v.trim();
            if let Some(stripped) = val.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                val = stripped;
            } else if let Some(stripped) = val.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))
            {
                val = stripped;
            }
            map.insert(key, val.to_string());
        }
    }
    Ok(map)
}

/// Read optional `projectId` from `m365agents.yml` if present.
fn read_project_id_from_yaml(project_dir: &Path) -> Option<String> {
    let yml_path = project_dir.join("m365agents.yml");
    if !yml_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(yml_path).ok()?;
    let value: serde_json::Value = serde_yaml_ng::from_str(&content).ok()?;
    value.get("projectId")?.as_str().map(ToString::to_string)
}

/// Read and decrypt the ATK project environment files.
///
/// Looks for `.env.residuum` and `.env.residuum.user` in `project_dir/env/`
/// or `project_dir/`.
///
/// # Errors
///
/// Returns [`FatalError::Config`] if required files are missing, cannot be parsed,
/// are missing required variables, or if decryption of the bot password fails.
pub fn read_atk_project_env(project_dir: &Path) -> Result<AtkProjectEnv, FatalError> {
    let env_file = if project_dir.join("env/.env.residuum").exists() {
        project_dir.join("env/.env.residuum")
    } else if project_dir.join(".env.residuum").exists() {
        project_dir.join(".env.residuum")
    } else {
        return Err(FatalError::Config(format!(
            "cannot find .env.residuum in {}",
            project_dir.display()
        )));
    };

    let user_env_file = if project_dir.join("env/.env.residuum.user").exists() {
        project_dir.join("env/.env.residuum.user")
    } else if project_dir.join(".env.residuum.user").exists() {
        project_dir.join(".env.residuum.user")
    } else {
        return Err(FatalError::Config(format!(
            "cannot find .env.residuum.user in {}",
            project_dir.display()
        )));
    };

    let env_vars = parse_env_file(&env_file)?;
    let user_vars = parse_env_file(&user_env_file)?;

    let bot_id = env_vars
        .get("BOT_ID")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| FatalError::Config("missing BOT_ID in .env.residuum".to_string()))?;

    let teams_app_tenant_id = env_vars
        .get("TEAMS_APP_TENANT_ID")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            FatalError::Config("missing TEAMS_APP_TENANT_ID in .env.residuum".to_string())
        })?;

    let teams_app_id = env_vars
        .get("TEAMS_APP_ID")
        .cloned()
        .filter(|s| !s.trim().is_empty());

    let encrypted_password = user_vars
        .get("SECRET_BOT_PASSWORD")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            FatalError::Config("missing SECRET_BOT_PASSWORD in .env.residuum.user".to_string())
        })?;

    let project_id = read_project_id_from_yaml(project_dir);
    let bot_password = decrypt_atk_secret(&encrypted_password, project_id.as_deref())
        .map_err(|e| FatalError::Config(e.to_string()))?;

    Ok(AtkProjectEnv {
        bot_id,
        teams_app_tenant_id,
        teams_app_id,
        bot_password,
    })
}

/// Import an ATK project into Residuum:
///
/// 1. Reads `.env.residuum` and `.env.residuum.user` from `project_dir`.
/// 2. Decrypts `SECRET_BOT_PASSWORD`.
/// 3. Stores `teams` secret in `SecretStore` at `hub_dir`.
/// 4. Updates `[teams]` in the agent's `config.toml` through a checkpointed write.
/// 5. Validates the updated configuration.
///
/// # Errors
///
/// Returns [`FatalError::Config`] if the agent directory does not exist, the ATK env
/// files cannot be read or decrypted, the patched config is invalid, or writing fails.
pub async fn import_atk_project(
    project_dir: &Path,
    agent_name: &str,
    residuum_root: &Path,
) -> Result<ImportAtkResult, FatalError> {
    let hub_path = hub_dir(residuum_root);
    let agent_path = agent_dir(residuum_root, agent_name);

    if !agent_path.exists() {
        return Err(FatalError::Config(format!(
            "agent directory '{}' does not exist",
            agent_path.display()
        )));
    }

    let project_env = read_atk_project_env(project_dir)?;
    let checkpoints = CheckpointEngine::open_for_cli(&hub_path);

    // 1. Checkpoint hub config and save secret in SecretStore
    if let Some(ref engine) = checkpoints {
        engine
            .checkpoint_config_before_write(CheckpointContext::system(
                CheckpointTrigger::PreConfigWrite,
                format!("import Teams secret for agent '{agent_name}'"),
            ))
            .await;
    }
    let mut secret_store = SecretStore::load(&hub_path)?;
    secret_store.set("teams", &project_env.bot_password, &hub_path)?;

    // 2. Read agent config.toml and apply patch
    let config_path = agent_path.join("config.toml");
    let existing_toml = match tokio::fs::read_to_string(&config_path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(FatalError::Config(format!(
                "failed to read {}: {e}",
                config_path.display()
            )));
        }
    };

    let patch = serde_json::json!({
        "teams": {
            "app_id": project_env.bot_id,
            "tenant_id": project_env.teams_app_tenant_id,
            "app_password": "secret:teams"
        }
    });

    let patch_map = patch
        .as_object()
        .ok_or_else(|| FatalError::Config("failed to construct teams config patch".to_string()))?;
    let patched_toml = crate::config::patch::apply_patch(&existing_toml, patch_map, "config.toml")
        .map_err(FatalError::Config)?;

    // 3. Validate patched config against hub config if loadable
    if let Ok(hub) = HubConfig::load_at(&hub_path) {
        Config::validate_agent_toml(&patched_toml, &agent_path, agent_name, &hub)
            .map_err(FatalError::Config)?;
    }

    // 4. Checkpoint agent config repo before writing
    if let Some(ref engine) = checkpoints {
        let _ = engine
            .checkpoint_config_kind_id_before_write(
                RepoKind::AgentConfig,
                CheckpointContext::system(
                    CheckpointTrigger::PreConfigWrite,
                    format!("configure Teams bot for agent '{agent_name}'"),
                ),
            )
            .await;
    }

    crate::util::fs::atomic_write(&config_path, &patched_toml).await?;

    Ok(ImportAtkResult {
        bot_id: project_env.bot_id,
        tenant_id: project_env.teams_app_tenant_id,
        teams_app_id: project_env.teams_app_id,
    })
}

/// Helper to forward an OAuth redirect URL to the local ATK login listener.
///
/// Validates that the URL points strictly to `localhost` or `127.0.0.1` and carries a port,
/// then issues an HTTP GET to complete the MSAL login code exchange.
///
/// # Errors
///
/// Returns [`FatalError::Config`] if the URL is invalid, does not point to localhost,
/// is missing a port, or the HTTP request fails.
pub async fn forward_redirect(url_str: &str) -> Result<u16, FatalError> {
    let parsed_url = url::Url::parse(url_str)
        .map_err(|e| FatalError::Config(format!("invalid redirect URL: {e}")))?;

    let host = parsed_url.host_str().unwrap_or("");
    if host != "localhost" && host != "127.0.0.1" {
        return Err(FatalError::Config(format!(
            "invalid redirect host '{host}': only localhost / 127.0.0.1 URLs can be forwarded"
        )));
    }

    let port = parsed_url.port().ok_or_else(|| {
        FatalError::Config(
            "redirect URL must specify a port (e.g. http://localhost:35437/...)".to_string(),
        )
    })?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| FatalError::Config(format!("failed to build HTTP client: {e}")))?;

    let response = client
        .get(parsed_url)
        .send()
        .await
        .map_err(|e| {
            FatalError::Config(format!(
                "failed to connect to local listener at localhost:{port}: {e}. Is 'atk auth login m365' still waiting?"
            ))
        })?;

    Ok(response.status().as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verified fixtures generated with pinned @microsoft/teamsfx-core@3.1.3:
    // Plaintext: "TestBotPassword123!"
    // Project ID: "test-project-123"
    // Salt: 64 bytes of 0x42
    // IV: 16 bytes of 0x24
    const FIXTURE_GLOBAL_CIPHERTEXT: &str = "crypto_42424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242242424242424242424242424242424241c2cf2ab17aa3b23ab952981303de07f9f6d6c37965d8dbdb24bb2f7119299bd5cffe1";
    const FIXTURE_PROJECT_CIPHERTEXT: &str = "crypto_4242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424224242424242424242424242424242424d78f4561013ca364f6a6e0e2d2f82d16da4108fc35c7f9d1977bdb3e9f58d2f2a6826b";

    #[test]
    fn decrypt_global_key_fixture() {
        let plain = decrypt_atk_secret(FIXTURE_GLOBAL_CIPHERTEXT, None).unwrap();
        assert_eq!(plain, "TestBotPassword123!");
    }

    #[test]
    fn decrypt_project_fallback_fixture() {
        // Without project_id, fallback ciphertext fails
        let err = decrypt_atk_secret(FIXTURE_PROJECT_CIPHERTEXT, None).unwrap_err();
        assert!(matches!(err, AtkDecryptError::DecryptionFailed(_)));

        // With wrong project_id, it fails
        let err_wrong =
            decrypt_atk_secret(FIXTURE_PROJECT_CIPHERTEXT, Some("wrong-project")).unwrap_err();
        assert!(matches!(err_wrong, AtkDecryptError::DecryptionFailed(_)));

        // With correct project_id, it succeeds
        let plain =
            decrypt_atk_secret(FIXTURE_PROJECT_CIPHERTEXT, Some("test-project-123")).unwrap();
        assert_eq!(plain, "TestBotPassword123!");
    }

    #[test]
    fn decrypt_unencrypted_legacy_string() {
        let raw = "legacy-plaintext-password-123";
        let plain = decrypt_atk_secret(raw, None).unwrap();
        assert_eq!(plain, raw);
    }

    #[test]
    fn decrypt_invalid_hex() {
        let invalid = "crypto_not-hex-characters";
        let err = decrypt_atk_secret(invalid, None).unwrap_err();
        assert!(matches!(err, AtkDecryptError::InvalidFormat(_)));
    }

    #[test]
    fn decrypt_truncated_payload() {
        // Less than 96 bytes (192 hex chars)
        let short = "crypto_42424242";
        let err = decrypt_atk_secret(short, None).unwrap_err();
        assert!(matches!(err, AtkDecryptError::InvalidFormat(_)));
    }

    #[test]
    fn decrypt_tampered_tag_fails() {
        // Corrupt tag byte
        let mut tampered = FIXTURE_GLOBAL_CIPHERTEXT.to_string();
        // Tag starts at index 7 + 160 = 167 (hex chars)
        tampered.replace_range(170..172, "00");
        let err = decrypt_atk_secret(&tampered, None).unwrap_err();
        assert!(matches!(err, AtkDecryptError::DecryptionFailed(_)));
    }

    #[test]
    fn parse_env_file_basic() {
        let temp = tempfile::tempdir().unwrap();
        let env_path = temp.path().join(".env.residuum");
        std::fs::write(
            &env_path,
            "# Comment line\nBOT_ID=bot-guid-123\nTEAMS_APP_TENANT_ID=\"tenant-guid-456\"\nEMPTY=\n",
        )
        .unwrap();

        let parsed = parse_env_file(&env_path).unwrap();
        assert_eq!(
            parsed.get("BOT_ID").map(String::as_str),
            Some("bot-guid-123")
        );
        assert_eq!(
            parsed.get("TEAMS_APP_TENANT_ID").map(String::as_str),
            Some("tenant-guid-456")
        );
        assert_eq!(parsed.get("EMPTY").map(String::as_str), Some(""));
    }

    #[tokio::test]
    async fn forward_redirect_validates_host() {
        // Non-localhost rejected
        let err = forward_redirect("http://example.com:3000/?code=123")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid redirect host"));

        // Missing port rejected
        let err_port = forward_redirect("http://localhost/?code=123")
            .await
            .unwrap_err();
        assert!(err_port.to_string().contains("must specify a port"));
    }

    #[tokio::test]
    async fn import_atk_project_updates_secret_and_config() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let hub_path = residuum_root.join("hub");
        let agent_path = residuum_root.join("test-agent");

        tokio::fs::create_dir_all(&hub_path).await.unwrap();
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        // Write initial hub config and agent config
        tokio::fs::write(hub_path.join("config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();

        tokio::fs::write(
            agent_path.join("config.toml"),
            "# Initial agent config\n[agent]\nname = \"test-agent\"\n",
        )
        .await
        .unwrap();

        // Scaffold ATK project
        let project_dir = agent_path.join("teams-app");
        let env_dir = project_dir.join("env");
        tokio::fs::create_dir_all(&env_dir).await.unwrap();

        tokio::fs::write(
            env_dir.join(".env.residuum"),
            "BOT_ID=11111111-2222-3333-4444-555555555555\nTEAMS_APP_TENANT_ID=tenant-guid-777\nTEAMS_APP_ID=teams-app-888\n",
        )
        .await
        .unwrap();

        tokio::fs::write(
            env_dir.join(".env.residuum.user"),
            format!("SECRET_BOT_PASSWORD={FIXTURE_GLOBAL_CIPHERTEXT}\n"),
        )
        .await
        .unwrap();

        // Run import
        let result = import_atk_project(&project_dir, "test-agent", &residuum_root)
            .await
            .unwrap();

        assert_eq!(result.bot_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(result.tenant_id, "tenant-guid-777");
        assert_eq!(result.teams_app_id.as_deref(), Some("teams-app-888"));

        // Verify SecretStore has decrypted password
        let store = SecretStore::load(&hub_path).unwrap();
        assert_eq!(store.get("teams"), Some("TestBotPassword123!"));

        // Verify config.toml was patched with [teams]
        let patched = tokio::fs::read_to_string(agent_path.join("config.toml"))
            .await
            .unwrap();
        assert!(patched.contains("[teams]"));
        assert!(patched.contains("app_id = \"11111111-2222-3333-4444-555555555555\""));
        assert!(patched.contains("tenant_id = \"tenant-guid-777\""));
        assert!(patched.contains("app_password = \"secret:teams\""));
        assert!(patched.contains("name = \"test-agent\""));
    }
}
