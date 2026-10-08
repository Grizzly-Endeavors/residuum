//! Microsoft 365 Agents Toolkit (ATK) credential decryption and project import.
//!
//! Provides routines to decrypt the encrypted bot password produced by ATK's
//! `writeToEnvironmentFile` driver (using Cryptr / AES-256-GCM), parse ATK project
//! outputs, store the decrypted secret in Residuum's [`SecretStore`], and configure
//! the agent's `[teams]` section via the checkpointed config path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aes::Aes256;
use aes_gcm::AesGcm;
use aes_gcm::aead::consts::U16;
use aes_gcm::aead::{Aead, KeyInit, Nonce};
use ring::pbkdf2;

type Aes256Gcm16 = AesGcm<Aes256, U16>;

use crate::checkpoints::{CheckpointContext, CheckpointEngine, CheckpointTrigger, RepoKind};
use crate::config::paths::{agent_dir, hub_dir, team_dir};
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

/// Embedded template for `m365agents.yml`.
pub const TEAMS_SETUP_M365AGENTS_YML: &str =
    include_str!("../../../assets/bundled-skills/teams-setup/templates/m365agents.yml");

/// Embedded template for `appPackage/manifest.json`.
pub const TEAMS_SETUP_MANIFEST_JSON: &str =
    include_str!("../../../assets/bundled-skills/teams-setup/templates/appPackage/manifest.json");

/// Embedded default 192x192 PNG color icon.
pub const TEAMS_SETUP_COLOR_PNG: &[u8] =
    include_bytes!("../../../assets/bundled-skills/teams-setup/templates/appPackage/color.png");

/// Embedded default 32x32 PNG outline icon.
pub const TEAMS_SETUP_OUTLINE_PNG: &[u8] =
    include_bytes!("../../../assets/bundled-skills/teams-setup/templates/appPackage/outline.png");

/// Embedded template for `env/.env.residuum`.
pub const TEAMS_SETUP_ENV_RESIDUUM: &str =
    include_str!("../../../assets/bundled-skills/teams-setup/templates/env/.env.residuum");

/// Validate that `bytes` begins with a valid PNG signature and has an IHDR chunk
/// matching `expected_w` and `expected_h`.
///
/// # Errors
///
/// Returns [`FatalError::Config`] if the image is not a valid PNG or has mismatched dimensions.
pub fn validate_png_dimensions(
    bytes: &[u8],
    expected_w: u32,
    expected_h: u32,
    label: &str,
) -> Result<(), FatalError> {
    const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || bytes.get(..8) != Some(PNG_MAGIC.as_slice()) {
        return Err(FatalError::Config(format!(
            "invalid {label}: not a valid PNG file (invalid header)"
        )));
    }
    if bytes.get(12..16) != Some(b"IHDR") {
        return Err(FatalError::Config(format!(
            "invalid {label}: missing IHDR chunk"
        )));
    }
    let width_bytes: [u8; 4] = bytes
        .get(16..20)
        .ok_or_else(|| FatalError::Config(format!("invalid {label}: corrupted width")))?
        .try_into()
        .map_err(|_err| FatalError::Config(format!("invalid {label}: corrupted width")))?;
    let height_bytes: [u8; 4] = bytes
        .get(20..24)
        .ok_or_else(|| FatalError::Config(format!("invalid {label}: corrupted height")))?
        .try_into()
        .map_err(|_err| FatalError::Config(format!("invalid {label}: corrupted height")))?;
    let width = u32::from_be_bytes(width_bytes);
    let height = u32::from_be_bytes(height_bytes);
    if width != expected_w || height != expected_h {
        return Err(FatalError::Config(format!(
            "{label} dimensions must be {expected_w}x{expected_h}, found {width}x{height}"
        )));
    }
    Ok(())
}

/// Quote and escape a value for use in a `.env` file according to standard dotenv rules.
///
/// Wraps value in double quotes and escapes `"` and `\`. Rejects values containing newline
/// characters (`\n` or `\r`).
///
/// # Errors
///
/// Returns [`FatalError::Config`] if the value contains newline characters.
pub fn format_dotenv_val(key: &str, val: &str) -> Result<String, FatalError> {
    if val.contains('\n') || val.contains('\r') {
        return Err(FatalError::Config(format!(
            "invalid value for '{key}': contains newline characters which are not allowed in .env files"
        )));
    }
    let escaped = val.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"{escaped}\""))
}

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

    // 2. AES-256-GCM with 16-byte nonce (AesGcm<Aes256, U16>)
    let cipher = Aes256Gcm16::new_from_slice(&key).map_err(|_err| ())?;
    let nonce = Nonce::<Aes256Gcm16>::from_slice(iv);

    // Reassemble ciphertext || tag for standard AEAD decryption
    let mut payload = Vec::with_capacity(enc.len().saturating_add(tag.len()));
    payload.extend_from_slice(enc);
    payload.extend_from_slice(tag);

    let decrypted = cipher
        .decrypt(nonce, payload.as_slice())
        .map_err(|_err| ())?;
    String::from_utf8(decrypted).map_err(|_err| ())
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
/// Paths associated with an agent's ATK project setup.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AtkPaths {
    /// Path to the teams-app project directory.
    pub project_dir: PathBuf,
    /// Path to the installed ATK binary inside `<hub>/tools/m365agentstoolkit`.
    pub atk_bin: PathBuf,
    /// Path to the built zip package for Teams deployment.
    pub package_zip: PathBuf,
    /// Path to the `.env.residuum` environment file.
    pub env_file: PathBuf,
}

/// Resolve standard ATK paths for an agent.
#[must_use]
pub fn resolve_atk_paths(residuum_root: &Path, agent_name: &str) -> AtkPaths {
    let hub_path = hub_dir(residuum_root);
    let agent_path = agent_dir(residuum_root, agent_name);
    let project_dir = agent_path.join("teams-app");
    let atk_bin_name = if cfg!(windows) { "atk.cmd" } else { "atk" };
    let atk_bin = hub_path
        .join("tools")
        .join("m365agentstoolkit")
        .join("node_modules")
        .join(".bin")
        .join(atk_bin_name);
    let package_zip = project_dir
        .join("appPackage")
        .join("build")
        .join("appPackage.residuum.zip");
    let env_file = project_dir.join("env").join(".env.residuum");
    AtkPaths {
        project_dir,
        atk_bin,
        package_zip,
        env_file,
    }
}

/// Query the local gateway to derive the cloud Teams messaging endpoint for `agent_name`.
///
/// Returns `Some(https://<instance host>/teams/<agent_name>)` if the hub is running and
/// connected to Residuum Cloud, or `None` if unreachable or not connected.
pub async fn derive_cloud_teams_endpoint(residuum_root: &Path, agent_name: &str) -> Option<String> {
    let hub_path = hub_dir(residuum_root);
    let gateway_addr = HubConfig::load_at_for_start(&hub_path, residuum_root).map_or_else(
        |_| crate::config::GatewayConfig::default().addr(),
        |hub| hub.gateway.addr(),
    );
    derive_cloud_teams_endpoint_from_addr(&gateway_addr, agent_name).await
}

/// Query a specific gateway address to derive the cloud Teams messaging endpoint.
pub async fn derive_cloud_teams_endpoint_from_addr(
    gateway_addr: &str,
    agent_name: &str,
) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct CloudStatus {
        instance_origin: Option<String>,
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;
    let url = format!("http://{gateway_addr}/api/hub/cloud/status");
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let status: CloudStatus = resp.json().await.ok()?;
    cloud_teams_endpoint(status.instance_origin.as_deref(), agent_name)
}

/// The Teams messaging endpoint for `agent_name`: the instance's own host the
/// tunnel announced, plus `/teams/<agent_name>`.
fn cloud_teams_endpoint(instance_origin: Option<&str>, agent_name: &str) -> Option<String> {
    let instance_origin = instance_origin
        .map(|v| v.trim().trim_end_matches('/'))
        .filter(|v| !v.is_empty())?;
    Some(format!("{instance_origin}/teams/{agent_name}"))
}

/// Options for scaffolding an ATK project for an agent.
#[derive(Debug, Clone)]
pub struct AtkScaffoldOptions {
    /// Agent name.
    pub agent_name: String,
    /// Bot messaging endpoint URL.
    pub endpoint: String,
    /// Custom project directory (defaults to `<agent>/teams-app`).
    pub project_dir: Option<PathBuf>,
    /// Allow overwriting existing files.
    pub force: bool,
    /// Bot display name (defaults to `agent_name`).
    pub bot_name: Option<String>,
    /// Developer name for manifest (defaults to "Residuum").
    pub developer_name: Option<String>,
    /// Developer website URL for manifest.
    pub developer_url: Option<String>,
    /// Privacy URL for manifest.
    pub privacy_url: Option<String>,
    /// Terms of use URL for manifest.
    pub terms_url: Option<String>,
    /// Short description for manifest.
    pub short_description: Option<String>,
    /// Long description for manifest.
    pub long_description: Option<String>,
    /// Optional path to custom 192x192 PNG color icon.
    pub color_icon: Option<PathBuf>,
    /// Optional path to custom 32x32 PNG outline icon.
    pub outline_icon: Option<PathBuf>,
}

/// Result of scaffolding an ATK project.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ScaffoldAtkResult {
    /// Absolute path to the scaffolded project directory.
    pub project_dir: PathBuf,
    /// Absolute path to the ATK CLI binary.
    pub atk_bin: PathBuf,
    /// Absolute path to the package zip file once built.
    pub package_zip: PathBuf,
    /// Absolute path to the written `.env.residuum` file.
    pub env_file: PathBuf,
    /// Bot endpoint configured in `.env.residuum`.
    pub endpoint: String,
}

/// Scaffold a Microsoft 365 Agents Toolkit project for an agent.
///
/// Copies templates from `<team>/skills/teams-setup/templates/` falling back to embedded templates,
/// generates `.env.residuum` with properly escaped values, validates PNG icons, and verifies
/// that existing files are not overwritten unless `force` is set.
///
/// # Errors
///
async fn resolve_template_text(
    team_path: &Path,
    embedded: &str,
    label: &str,
) -> Result<String, FatalError> {
    if team_path.is_file() {
        tokio::fs::read_to_string(team_path)
            .await
            .map_err(|e| FatalError::Config(format!("failed to read template {label}: {e}")))
    } else {
        Ok(embedded.to_string())
    }
}

async fn resolve_icon_bytes(
    custom_icon: Option<&Path>,
    team_path: &Path,
    embedded: &[u8],
    (w, h): (u32, u32),
    label: &str,
) -> Result<Vec<u8>, FatalError> {
    if let Some(path) = custom_icon {
        let bytes = tokio::fs::read(path).await.map_err(|e| {
            FatalError::Config(format!(
                "failed to read {label} from {}: {e}",
                path.display()
            ))
        })?;
        validate_png_dimensions(&bytes, w, h, label)?;
        Ok(bytes)
    } else if team_path.is_file() {
        let bytes = tokio::fs::read(team_path)
            .await
            .map_err(|e| FatalError::Config(format!("failed to read template {label}: {e}")))?;
        validate_png_dimensions(&bytes, w, h, label)?;
        Ok(bytes)
    } else {
        validate_png_dimensions(embedded, w, h, label)?;
        Ok(embedded.to_vec())
    }
}

fn build_scaffold_env(options: &AtkScaffoldOptions) -> Result<String, FatalError> {
    let bot_display = options.bot_name.as_deref().unwrap_or(&options.agent_name);
    let dev_name = options.developer_name.as_deref().unwrap_or("Residuum");
    let dev_url = options
        .developer_url
        .as_deref()
        .unwrap_or("https://github.com/Grizzly-Endeavors/residuum");
    let priv_url = options
        .privacy_url
        .as_deref()
        .unwrap_or("https://github.com/Grizzly-Endeavors/residuum");
    let terms_url = options
        .terms_url
        .as_deref()
        .unwrap_or("https://github.com/Grizzly-Endeavors/residuum");
    let short_desc = options
        .short_description
        .as_deref()
        .unwrap_or("Residuum Agent in Microsoft Teams");
    let long_desc = options
        .long_description
        .as_deref()
        .unwrap_or("Personal AI agent gateway integration with Microsoft Teams");

    let lines = [
        "TEAMSFX_ENV=residuum".to_string(),
        format!(
            "BOT_DISPLAY_NAME={}",
            format_dotenv_val("BOT_DISPLAY_NAME", bot_display)?
        ),
        format!(
            "TEAMS_APP_NAME={}",
            format_dotenv_val("TEAMS_APP_NAME", bot_display)?
        ),
        format!(
            "BOT_ENDPOINT={}",
            format_dotenv_val("BOT_ENDPOINT", &options.endpoint)?
        ),
        format!(
            "DEVELOPER_NAME={}",
            format_dotenv_val("DEVELOPER_NAME", dev_name)?
        ),
        format!(
            "DEVELOPER_URL={}",
            format_dotenv_val("DEVELOPER_URL", dev_url)?
        ),
        format!(
            "PRIVACY_URL={}",
            format_dotenv_val("PRIVACY_URL", priv_url)?
        ),
        format!("TERMS_URL={}", format_dotenv_val("TERMS_URL", terms_url)?),
        format!(
            "SHORT_DESCRIPTION={}",
            format_dotenv_val("SHORT_DESCRIPTION", short_desc)?
        ),
        format!(
            "LONG_DESCRIPTION={}",
            format_dotenv_val("LONG_DESCRIPTION", long_desc)?
        ),
    ];
    Ok(format!("{}\n", lines.join("\n")))
}

/// Scaffold a Microsoft 365 Agents Toolkit project for an agent.
///
/// Copies templates from `<team>/skills/teams-setup/templates/` falling back to embedded templates,
/// generates `.env.residuum` with properly escaped values, validates PNG icons, and verifies
/// that existing files are not overwritten unless `force` is set.
///
/// # Errors
fn check_overwrite_safety(
    project_dir: &Path,
    target_files: &[PathBuf],
    force: bool,
) -> Result<(), FatalError> {
    if force {
        return Ok(());
    }
    let existing: Vec<_> = target_files.iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        return Ok(());
    }
    let names = existing
        .iter()
        .map(|p| p.file_name().unwrap_or_default().to_string_lossy())
        .collect::<Vec<_>>()
        .join(", ");
    Err(FatalError::Config(format!(
        "refusing to overwrite existing ATK project files in '{}' ({names}); pass --force to overwrite",
        project_dir.display()
    )))
}

struct ScaffoldPayload<'a> {
    m365_content: &'a str,
    manifest_content: &'a str,
    color_bytes: &'a [u8],
    outline_bytes: &'a [u8],
    env_content: &'a str,
}

async fn write_scaffold_project(
    project_dir: &Path,
    payload: ScaffoldPayload<'_>,
) -> Result<(), FatalError> {
    let app_package_dir = project_dir.join("appPackage");
    let env_dir = project_dir.join("env");

    tokio::fs::create_dir_all(&app_package_dir)
        .await
        .map_err(|e| {
            FatalError::Config(format!(
                "failed to create directory {}: {e}",
                app_package_dir.display()
            ))
        })?;
    tokio::fs::create_dir_all(&env_dir).await.map_err(|e| {
        FatalError::Config(format!(
            "failed to create directory {}: {e}",
            env_dir.display()
        ))
    })?;

    crate::util::fs::atomic_write(&project_dir.join("m365agents.yml"), payload.m365_content)
        .await?;
    crate::util::fs::atomic_write(
        &app_package_dir.join("manifest.json"),
        payload.manifest_content,
    )
    .await?;
    tokio::fs::write(app_package_dir.join("color.png"), payload.color_bytes)
        .await
        .map_err(|e| FatalError::Config(format!("failed to write color.png: {e}")))?;
    tokio::fs::write(app_package_dir.join("outline.png"), payload.outline_bytes)
        .await
        .map_err(|e| FatalError::Config(format!("failed to write outline.png: {e}")))?;
    crate::util::fs::atomic_write(&env_dir.join(".env.residuum"), payload.env_content).await?;
    Ok(())
}

/// Scaffold a Microsoft 365 Agents Toolkit project for an agent.
///
/// Copies templates from `<team>/skills/teams-setup/templates/` falling back to embedded templates,
/// generates `.env.residuum` with properly escaped values, validates PNG icons, and verifies
/// that existing files are not overwritten unless `force` is set.
///
/// # Errors
///
/// Returns [`FatalError::Config`] if the agent name is invalid, the agent directory does not exist,
/// files already exist without `force`, icon dimensions are invalid, or writes fail.
pub async fn scaffold_atk_project(
    residuum_root: &Path,
    options: &AtkScaffoldOptions,
) -> Result<ScaffoldAtkResult, FatalError> {
    crate::config::paths::validate_agent_name(&options.agent_name).map_err(FatalError::Config)?;

    let agent_path = agent_dir(residuum_root, &options.agent_name);
    if !agent_path.exists() {
        return Err(FatalError::Config(format!(
            "agent directory '{}' does not exist",
            agent_path.display()
        )));
    }

    let project_dir = options
        .project_dir
        .clone()
        .unwrap_or_else(|| agent_path.join("teams-app"));

    check_overwrite_safety(
        &project_dir,
        &[
            project_dir.join("m365agents.yml"),
            project_dir.join("appPackage/manifest.json"),
            project_dir.join("appPackage/color.png"),
            project_dir.join("appPackage/outline.png"),
            project_dir.join("env/.env.residuum"),
        ],
        options.force,
    )?;

    let team_templates = team_dir(residuum_root)
        .join("skills")
        .join("teams-setup")
        .join("templates");

    let m365_content = resolve_template_text(
        &team_templates.join("m365agents.yml"),
        TEAMS_SETUP_M365AGENTS_YML,
        "m365agents.yml",
    )
    .await?;

    let manifest_content = resolve_template_text(
        &team_templates.join("appPackage").join("manifest.json"),
        TEAMS_SETUP_MANIFEST_JSON,
        "manifest.json",
    )
    .await?;

    let color_bytes = resolve_icon_bytes(
        options.color_icon.as_deref(),
        &team_templates.join("appPackage").join("color.png"),
        TEAMS_SETUP_COLOR_PNG,
        (192, 192),
        "color icon",
    )
    .await?;

    let outline_bytes = resolve_icon_bytes(
        options.outline_icon.as_deref(),
        &team_templates.join("appPackage").join("outline.png"),
        TEAMS_SETUP_OUTLINE_PNG,
        (32, 32),
        "outline icon",
    )
    .await?;

    let env_content = build_scaffold_env(options)?;

    write_scaffold_project(
        &project_dir,
        ScaffoldPayload {
            m365_content: &m365_content,
            manifest_content: &manifest_content,
            color_bytes: &color_bytes,
            outline_bytes: &outline_bytes,
            env_content: &env_content,
        },
    )
    .await?;

    let paths = resolve_atk_paths(residuum_root, &options.agent_name);
    let package_zip = project_dir
        .join("appPackage")
        .join("build")
        .join("appPackage.residuum.zip");

    Ok(ScaffoldAtkResult {
        project_dir: project_dir.clone(),
        atk_bin: paths.atk_bin,
        package_zip,
        env_file: project_dir.join("env/.env.residuum"),
        endpoint: options.endpoint.clone(),
    })
}

/// Import an ATK project into Residuum:
///
/// 1. Reads `.env.residuum` and `.env.residuum.user` from `project_dir`.
/// 2. Decrypts `SECRET_BOT_PASSWORD`.
/// 3. Validates patched configuration against `HubConfig`.
/// 4. Stores `teams` secret in `SecretStore` at `hub_dir`.
/// 5. Updates `[teams]` in the agent's `config/config.toml` through a checkpointed write.
///
/// # Errors
///
/// Returns [`FatalError::Config`] if the agent directory does not exist, the ATK env
/// files cannot be read or decrypted, the hub config cannot be loaded, the patched
/// config fails validation, or writing fails.
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

    // Determine config path: prefer canonical <agent>/config/config.toml, fallback to <agent>/config.toml
    let canonical_config = agent_path.join("config").join("config.toml");
    let legacy_config = agent_path.join("config.toml");
    let config_path = if canonical_config.exists() {
        canonical_config
    } else if legacy_config.exists() {
        legacy_config
    } else if agent_path.join("config").is_dir() {
        canonical_config
    } else {
        legacy_config
    };

    let project_env = read_atk_project_env(project_dir)?;

    // 1. Load hub config for validation; do not skip validation if loading fails!
    let hub = HubConfig::load_at(&hub_path).map_err(|e| {
        tracing::error!(hub_path = %hub_path.display(), error = %e, "failed to load hub configuration for validation");
        FatalError::Config(format!(
            "failed to load hub configuration at {}: {e}",
            hub_path.display()
        ))
    })?;

    // 2. Read agent config.toml and apply patch
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

    // 3. Validate patched config BEFORE saving secret or modifying disk
    Config::validate_agent_toml(&patched_toml, &agent_path, agent_name, &hub)
        .map_err(FatalError::Config)?;

    let checkpoints = CheckpointEngine::open_for_cli(&hub_path);

    // 4. Checkpoint hub config and save secret in SecretStore
    if let Some(ref engine) = checkpoints {
        let hub_cp = engine
            .checkpoint_config_id_before_write(CheckpointContext::system(
                CheckpointTrigger::PreConfigWrite,
                format!("import Teams secret for agent '{agent_name}'"),
            ))
            .await;
        if hub_cp.is_none() {
            tracing::warn!(agent = %agent_name, "no hub config checkpoint recorded before saving Teams secret");
        } else {
            tracing::debug!(agent = %agent_name, checkpoint_id = ?hub_cp, "checkpointed hub config before saving Teams secret");
        }
    }
    let mut secret_store = SecretStore::load(&hub_path)?;
    secret_store.set("teams", &project_env.bot_password, &hub_path)?;

    // 5. Checkpoint agent config repo before writing
    if let Some(ref engine) = checkpoints {
        let config_id = engine
            .checkpoint_config_kind_id_before_write(
                RepoKind::AgentConfig,
                CheckpointContext::system(
                    CheckpointTrigger::PreConfigWrite,
                    format!("configure Teams bot for agent '{agent_name}'"),
                ),
            )
            .await;
        if config_id.is_none() {
            tracing::warn!(agent = %agent_name, "no agent config checkpoint recorded before writing Teams configuration");
        } else {
            tracing::debug!(agent = %agent_name, checkpoint_id = ?config_id, "checkpointed agent config before writing Teams configuration");
        }
    }

    // 6. Write patched config to disk
    if let Err(e) = crate::util::fs::atomic_write(&config_path, &patched_toml).await {
        return Err(FatalError::Config(format!(
            "failed to write {}: {e}. Secret 'teams' was stored in hub SecretStore, but agent config was not updated.",
            config_path.display()
        )));
    }

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
    #[test]
    fn the_teams_endpoint_is_on_the_instance_host() {
        use super::cloud_teams_endpoint;
        assert_eq!(
            cloud_teams_endpoint(Some("https://laptop.bear.agent-residuum.com/"), "scout")
                .as_deref(),
            Some("https://laptop.bear.agent-residuum.com/teams/scout")
        );
        assert_eq!(cloud_teams_endpoint(None, "scout"), None);
        assert_eq!(cloud_teams_endpoint(Some("  "), "scout"), None);
    }

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

    #[test]
    fn format_dotenv_val_escaping_and_newline_rejection() {
        assert_eq!(
            format_dotenv_val("TEST_KEY", "simple").unwrap(),
            "\"simple\""
        );
        assert_eq!(
            format_dotenv_val("TEST_KEY", "hello \"world\"").unwrap(),
            "\"hello \\\"world\\\"\""
        );
        assert_eq!(
            format_dotenv_val("TEST_KEY", "path\\to\\dir").unwrap(),
            "\"path\\\\to\\\\dir\""
        );

        let err_nl = format_dotenv_val("TEST_KEY", "hello\nworld").unwrap_err();
        assert!(err_nl.to_string().contains("contains newline characters"));

        let err_cr = format_dotenv_val("TEST_KEY", "hello\rworld").unwrap_err();
        assert!(err_cr.to_string().contains("contains newline characters"));
    }

    #[test]
    fn validate_png_dimensions_checks() {
        // Embedded PNGs should pass
        validate_png_dimensions(TEAMS_SETUP_COLOR_PNG, 192, 192, "color icon").unwrap();
        validate_png_dimensions(TEAMS_SETUP_OUTLINE_PNG, 32, 32, "outline icon").unwrap();

        // Mismatched dimensions fail
        let err_dim =
            validate_png_dimensions(TEAMS_SETUP_COLOR_PNG, 32, 32, "color icon").unwrap_err();
        assert!(
            err_dim
                .to_string()
                .contains("dimensions must be 32x32, found 192x192")
        );

        // Invalid header fails
        let err_hdr =
            validate_png_dimensions(b"not a png image at all", 192, 192, "bad icon").unwrap_err();
        assert!(err_hdr.to_string().contains("invalid header"));

        // Short slice fails
        let err_short =
            validate_png_dimensions(b"\x89PNG\r\n\x1a\n", 192, 192, "short icon").unwrap_err();
        assert!(err_short.to_string().contains("invalid header"));
    }

    #[tokio::test]
    async fn scaffold_atk_project_basic_and_overwrite_refusal() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let agent_path = residuum_root.join("scout");
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        let options = AtkScaffoldOptions {
            agent_name: "scout".to_string(),
            endpoint: "https://relay.example.com/teams/inst1/scout".to_string(),
            project_dir: None,
            force: false,
            bot_name: Some("Scout Assistant".to_string()),
            developer_name: Some("Grizzly Endeavors".to_string()),
            developer_url: None,
            privacy_url: None,
            terms_url: None,
            short_description: Some("Scout bot".to_string()),
            long_description: None,
            color_icon: None,
            outline_icon: None,
        };

        // First scaffold succeeds
        let res = scaffold_atk_project(&residuum_root, &options)
            .await
            .unwrap();
        assert_eq!(res.project_dir, agent_path.join("teams-app"));
        assert!(res.project_dir.join("m365agents.yml").exists());
        assert!(res.project_dir.join("appPackage/manifest.json").exists());
        assert!(res.project_dir.join("appPackage/color.png").exists());
        assert!(res.project_dir.join("appPackage/outline.png").exists());
        assert!(res.project_dir.join("env/.env.residuum").exists());

        // Verify .env.residuum content
        let env_content = tokio::fs::read_to_string(res.project_dir.join("env/.env.residuum"))
            .await
            .unwrap();
        assert!(env_content.contains("BOT_DISPLAY_NAME=\"Scout Assistant\""));
        assert!(
            env_content.contains("BOT_ENDPOINT=\"https://relay.example.com/teams/inst1/scout\"")
        );
        assert!(env_content.contains("DEVELOPER_NAME=\"Grizzly Endeavors\""));

        // Overwrite without force fails
        let err = scaffold_atk_project(&residuum_root, &options)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"));

        // Overwrite with force succeeds
        let mut force_opts = options.clone();
        force_opts.force = true;
        force_opts.bot_name = Some("Updated Scout".to_string());
        let res2 = scaffold_atk_project(&residuum_root, &force_opts)
            .await
            .unwrap();
        let env2 = tokio::fs::read_to_string(res2.project_dir.join("env/.env.residuum"))
            .await
            .unwrap();
        assert!(env2.contains("BOT_DISPLAY_NAME=\"Updated Scout\""));
    }

    #[tokio::test]
    async fn scaffold_atk_project_custom_icon_validation() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let agent_path = residuum_root.join("scout");
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        // Write bad icon file
        let bad_icon = temp.path().join("bad.png");
        tokio::fs::write(&bad_icon, b"not a valid png")
            .await
            .unwrap();

        let options = AtkScaffoldOptions {
            agent_name: "scout".to_string(),
            endpoint: "https://example.com/teams".to_string(),
            project_dir: None,
            force: false,
            bot_name: None,
            developer_name: None,
            developer_url: None,
            privacy_url: None,
            terms_url: None,
            short_description: None,
            long_description: None,
            color_icon: Some(bad_icon),
            outline_icon: None,
        };

        let err = scaffold_atk_project(&residuum_root, &options)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid color icon"));
    }

    #[tokio::test]
    async fn scaffold_atk_project_uses_team_template_override() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let agent_path = residuum_root.join("scout");
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        // Create team template override for m365agents.yml
        let team_templates = residuum_root.join("team/skills/teams-setup/templates");
        tokio::fs::create_dir_all(&team_templates).await.unwrap();
        tokio::fs::write(
            team_templates.join("m365agents.yml"),
            "# Custom team template override\nversion: v1.13\n",
        )
        .await
        .unwrap();

        let options = AtkScaffoldOptions {
            agent_name: "scout".to_string(),
            endpoint: "https://example.com/teams".to_string(),
            project_dir: None,
            force: false,
            bot_name: None,
            developer_name: None,
            developer_url: None,
            privacy_url: None,
            terms_url: None,
            short_description: None,
            long_description: None,
            color_icon: None,
            outline_icon: None,
        };

        let res = scaffold_atk_project(&residuum_root, &options)
            .await
            .unwrap();
        let m365_content = tokio::fs::read_to_string(res.project_dir.join("m365agents.yml"))
            .await
            .unwrap();
        assert!(m365_content.contains("# Custom team template override"));
    }

    #[tokio::test]
    async fn import_atk_project_uses_canonical_config_path() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let hub_path = residuum_root.join("hub");
        let agent_path = residuum_root.join("scout");
        let agent_config_dir = agent_path.join("config");

        tokio::fs::create_dir_all(&hub_path).await.unwrap();
        tokio::fs::create_dir_all(&agent_config_dir).await.unwrap();

        tokio::fs::write(hub_path.join("config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();
        tokio::fs::write(
            agent_config_dir.join("config.toml"),
            "[agent]\nname = \"scout\"\n",
        )
        .await
        .unwrap();

        let project_dir = agent_path.join("teams-app");
        let env_dir = project_dir.join("env");
        tokio::fs::create_dir_all(&env_dir).await.unwrap();

        tokio::fs::write(
            env_dir.join(".env.residuum"),
            "BOT_ID=bot-id-abc\nTEAMS_APP_TENANT_ID=tenant-id-def\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            env_dir.join(".env.residuum.user"),
            format!("SECRET_BOT_PASSWORD={FIXTURE_GLOBAL_CIPHERTEXT}\n"),
        )
        .await
        .unwrap();

        let res = import_atk_project(&project_dir, "scout", &residuum_root)
            .await
            .unwrap();
        assert_eq!(res.bot_id, "bot-id-abc");

        // Canonical config/config.toml was patched
        let patched = tokio::fs::read_to_string(agent_config_dir.join("config.toml"))
            .await
            .unwrap();
        assert!(patched.contains("[teams]"));
        assert!(patched.contains("app_id = \"bot-id-abc\""));
    }

    #[tokio::test]
    async fn import_atk_project_validates_before_storing_secret() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let hub_path = residuum_root.join("hub");
        let agent_path = residuum_root.join("scout");

        tokio::fs::create_dir_all(&hub_path).await.unwrap();
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        // Hub config has missing/invalid timezone so agent config validation will fail
        // or agent config has an invalid setting that fails Config::validate_agent_toml
        tokio::fs::write(hub_path.join("config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();
        // An invalid agent config toml (invalid type for timeout_secs: string instead of integer)
        tokio::fs::write(
            agent_path.join("config.toml"),
            "timeout_secs = \"not-a-number\"\n",
        )
        .await
        .unwrap();

        let project_dir = agent_path.join("teams-app");
        let env_dir = project_dir.join("env");
        tokio::fs::create_dir_all(&env_dir).await.unwrap();

        tokio::fs::write(
            env_dir.join(".env.residuum"),
            "BOT_ID=bot-id-abc\nTEAMS_APP_TENANT_ID=tenant-id-def\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            env_dir.join(".env.residuum.user"),
            format!("SECRET_BOT_PASSWORD={FIXTURE_GLOBAL_CIPHERTEXT}\n"),
        )
        .await
        .unwrap();

        // Import should fail on config validation
        let err = import_atk_project(&project_dir, "scout", &residuum_root)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid")
                || err.to_string().contains("expected a string")
                || err.to_string().contains("data did not match")
        );

        // Crucial check: SecretStore was NOT populated because validation failed FIRST!
        let store = SecretStore::load(&hub_path).unwrap();
        assert_eq!(store.get("teams"), None);
    }
}
