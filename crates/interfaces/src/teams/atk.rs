use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use aes::Aes256;
use aes_gcm::AesGcm;
use aes_gcm::aead::consts::U16;
use aes_gcm::aead::{Aead, KeyInit, Nonce};
use ring::pbkdf2;

type Aes256Gcm16 = AesGcm<Aes256, U16>;

use residuum_checkpoints::{CheckpointContext, CheckpointEngine, CheckpointTrigger, RepoKind};
use residuum_config::paths::{agent_dir, hub_dir, team_dir};
use residuum_config::secrets::SecretStore;
use residuum_config::{Config, HubConfig};
use residuum_core::util::FatalError;

pub const ATK_CLI_VERSION: &str = "1.1.17";

pub const ATK_DEFAULT_GLOBAL_KEY: &str = "teamsfx_global_key";

pub const CRYPTO_PREFIX: &str = "crypto_";

pub const TEAMS_SETUP_M365AGENTS_YML: &str =
    include_str!("../../../../assets/bundled-skills/teams-setup/templates/m365agents.yml");

pub const TEAMS_SETUP_MANIFEST_JSON: &str = include_str!(
    "../../../../assets/bundled-skills/teams-setup/templates/appPackage/manifest.json"
);

pub const TEAMS_SETUP_COLOR_PNG: &[u8] =
    include_bytes!("../../../../assets/bundled-skills/teams-setup/templates/appPackage/color.png");

pub const TEAMS_SETUP_OUTLINE_PNG: &[u8] = include_bytes!(
    "../../../../assets/bundled-skills/teams-setup/templates/appPackage/outline.png"
);

pub const TEAMS_SETUP_ENV_RESIDUUM: &str =
    include_str!("../../../../assets/bundled-skills/teams-setup/templates/env/.env.residuum");

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

pub fn format_dotenv_val(key: &str, val: &str) -> Result<String, FatalError> {
    if val.contains('\n') || val.contains('\r') {
        return Err(FatalError::Config(format!(
            "invalid value for '{key}': contains newline characters which are not allowed in .env files"
        )));
    }
    let escaped = val.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"{escaped}\""))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AtkDecryptError {
    #[error("encrypted secret has invalid format: {0}")]
    InvalidFormat(String),
    #[error(
        "decryption failed: cipher text is broken or key mismatch. You can create a new client secret manually in Microsoft Teams Developer Portal (https://dev.teams.microsoft.com/bots) and store it with `residuum secret set teams`."
    )]
    DecryptionFailed(String),
}

pub fn decrypt_atk_secret(
    ciphertext: &str,
    project_id: Option<&str>,
) -> Result<String, AtkDecryptError> {
    let Some(hex_part) = ciphertext.strip_prefix(CRYPTO_PREFIX) else {
        return Ok(ciphertext.to_string());
    };

    let raw = hex::decode(hex_part)
        .map_err(|e| AtkDecryptError::InvalidFormat(format!("invalid hex: {e}")))?;

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

    if let Ok(plain) = decrypt_gcm_with_secret(ATK_DEFAULT_GLOBAL_KEY, salt, iv, tag, enc) {
        return Ok(plain);
    }

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

fn decrypt_gcm_with_secret(
    secret: &str,
    salt: &[u8],
    iv: &[u8],
    tag: &[u8],
    enc: &[u8],
) -> Result<String, ()> {
    let mut key = [0_u8; 32];
    let iterations = std::num::NonZeroU32::new(100_000).ok_or(())?;
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA512,
        iterations,
        salt,
        secret.as_bytes(),
        &mut key,
    );

    let cipher = Aes256Gcm16::new_from_slice(&key).map_err(|_err| ())?;
    let nonce = Nonce::<Aes256Gcm16>::from_slice(iv);

    let mut payload = Vec::with_capacity(enc.len().saturating_add(tag.len()));
    payload.extend_from_slice(enc);
    payload.extend_from_slice(tag);

    let decrypted = cipher
        .decrypt(nonce, payload.as_slice())
        .map_err(|_err| ())?;
    String::from_utf8(decrypted).map_err(|_err| ())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtkProjectEnv {
    pub bot_id: String,
    pub teams_app_tenant_id: String,
    pub teams_app_id: Option<String>,
    pub bot_password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportAtkResult {
    pub bot_id: String,
    pub tenant_id: String,
    pub teams_app_id: Option<String>,
}

fn parse_env_file(path: &Path) -> Result<HashMap<String, String>, FatalError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        FatalError::Config(format!(
            "failed to read env file at {}: {e}",
            path.display()
        ))
    })?;
    Ok(parse_env_content(&content))
}

fn parse_env_content(content: &str) -> HashMap<String, String> {
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
    map
}

fn project_id_from_yaml_text(content: &str) -> Result<Option<String>, serde_yaml_ng::Error> {
    let value: serde_json::Value = serde_yaml_ng::from_str(content)?;
    Ok(value
        .get("projectId")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string))
}

fn read_project_id_from_yaml(project_dir: &Path) -> Option<String> {
    let yml_path = project_dir.join("m365agents.yml");
    if !yml_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(yml_path).ok()?;
    project_id_from_yaml_text(&content).ok().flatten()
}

async fn carry_over_project_id(
    template_text: String,
    existing_path: &Path,
) -> Result<String, FatalError> {
    let existing = match tokio::fs::read_to_string(existing_path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(template_text),
        Err(e) => {
            return Err(FatalError::Config(format!(
                "failed to read existing {}: {e}",
                existing_path.display()
            )));
        }
    };
    let project_id = match project_id_from_yaml_text(&existing) {
        Ok(Some(id)) => id,
        Ok(None) => return Ok(template_text),
        Err(e) => {
            tracing::warn!(
                path = %existing_path.display(),
                error = %e,
                "existing m365agents.yml is not valid YAML; refreshing it without carrying over projectId"
            );
            return Ok(template_text);
        }
    };
    if matches!(project_id_from_yaml_text(&template_text), Ok(Some(_))) {
        return Ok(template_text);
    }
    let rendered = serde_yaml_ng::to_string(&project_id).map_err(|e| {
        FatalError::Config(format!(
            "failed to keep the projectId from {}: {e}",
            existing_path.display()
        ))
    })?;
    let mut out = template_text;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("projectId: ");
    out.push_str(rendered.trim_end());
    out.push('\n');
    Ok(out)
}

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AtkPaths {
    pub project_dir: PathBuf,
    pub atk_bin: PathBuf,
    pub package_zip: PathBuf,
    pub env_file: PathBuf,
}

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

pub async fn derive_cloud_teams_endpoint(residuum_root: &Path, agent_name: &str) -> Option<String> {
    let hub_path = hub_dir(residuum_root);
    let gateway_addr = HubConfig::load_at_for_start(&hub_path, residuum_root).map_or_else(
        |_| residuum_config::GatewayConfig::default().addr(),
        |hub| hub.gateway.addr(),
    );
    derive_cloud_teams_endpoint_from_addr(&gateway_addr, agent_name).await
}

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

fn cloud_teams_endpoint(instance_origin: Option<&str>, agent_name: &str) -> Option<String> {
    let instance_origin = instance_origin
        .map(|v| v.trim().trim_end_matches('/'))
        .filter(|v| !v.is_empty())?;
    Some(format!("{instance_origin}/teams/{agent_name}"))
}

#[derive(Debug, Clone)]
pub struct AtkScaffoldOptions {
    pub agent_name: String,
    pub endpoint: String,
    pub project_dir: Option<PathBuf>,
    pub bot_name: Option<String>,
    pub developer_name: Option<String>,
    pub developer_url: Option<String>,
    pub privacy_url: Option<String>,
    pub terms_url: Option<String>,
    pub short_description: Option<String>,
    pub long_description: Option<String>,
    pub color_icon: Option<PathBuf>,
    pub outline_icon: Option<PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ScaffoldAtkResult {
    pub project_dir: PathBuf,
    pub atk_bin: PathBuf,
    pub package_zip: PathBuf,
    pub env_file: PathBuf,
    pub endpoint: String,
    pub reused_bot_id: Option<String>,
}

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

fn build_scaffold_env_entries(
    options: &AtkScaffoldOptions,
) -> Result<Vec<(&'static str, String)>, FatalError> {
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

    Ok(vec![
        ("TEAMSFX_ENV", "residuum".to_string()),
        (
            "BOT_DISPLAY_NAME",
            format_dotenv_val("BOT_DISPLAY_NAME", bot_display)?,
        ),
        (
            "TEAMS_APP_NAME",
            format_dotenv_val("TEAMS_APP_NAME", bot_display)?,
        ),
        (
            "BOT_ENDPOINT",
            format_dotenv_val("BOT_ENDPOINT", &options.endpoint)?,
        ),
        (
            "DEVELOPER_NAME",
            format_dotenv_val("DEVELOPER_NAME", dev_name)?,
        ),
        (
            "DEVELOPER_URL",
            format_dotenv_val("DEVELOPER_URL", dev_url)?,
        ),
        ("PRIVACY_URL", format_dotenv_val("PRIVACY_URL", priv_url)?),
        ("TERMS_URL", format_dotenv_val("TERMS_URL", terms_url)?),
        (
            "SHORT_DESCRIPTION",
            format_dotenv_val("SHORT_DESCRIPTION", short_desc)?,
        ),
        (
            "LONG_DESCRIPTION",
            format_dotenv_val("LONG_DESCRIPTION", long_desc)?,
        ),
    ])
}

fn merge_scaffold_env(existing: Option<&str>, entries: &[(&'static str, String)]) -> String {
    let mut written: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    for line in existing.unwrap_or_default().lines() {
        let trimmed = line.trim();
        let scaffold_entry = if trimmed.is_empty() || trimmed.starts_with('#') {
            None
        } else {
            trimmed
                .split_once('=')
                .and_then(|(k, _)| entries.iter().find(|(name, _)| *name == k.trim()))
        };
        match scaffold_entry {
            Some((name, value)) => {
                if written.insert(name) {
                    out.push(format!("{name}={value}"));
                }
            }
            None => out.push(line.to_string()),
        }
    }

    for (name, value) in entries {
        if written.insert(name) {
            out.push(format!("{name}={value}"));
        }
    }

    format!("{}\n", out.join("\n"))
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

    residuum_core::util::fs::atomic_write(
        &project_dir.join("m365agents.yml"),
        payload.m365_content,
    )
    .await?;
    residuum_core::util::fs::atomic_write(
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
    residuum_core::util::fs::atomic_write(&env_dir.join(".env.residuum"), payload.env_content)
        .await?;
    Ok(())
}

pub async fn scaffold_atk_project(
    residuum_root: &Path,
    options: &AtkScaffoldOptions,
) -> Result<ScaffoldAtkResult, FatalError> {
    residuum_config::paths::validate_agent_name(&options.agent_name).map_err(FatalError::Config)?;

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

    let team_templates = team_dir(residuum_root)
        .join("skills")
        .join("teams-setup")
        .join("templates");

    let m365_template = resolve_template_text(
        &team_templates.join("m365agents.yml"),
        TEAMS_SETUP_M365AGENTS_YML,
        "m365agents.yml",
    )
    .await?;
    let m365_content =
        carry_over_project_id(m365_template, &project_dir.join("m365agents.yml")).await?;

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

    let env_entries = build_scaffold_env_entries(options)?;
    let env_path = project_dir.join("env/.env.residuum");
    let existing_env = match tokio::fs::read_to_string(&env_path).await {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(FatalError::Config(format!(
                "failed to read existing {}: {e}",
                env_path.display()
            )));
        }
    };
    let env_content = merge_scaffold_env(existing_env.as_deref(), &env_entries);
    let reused_bot_id = parse_env_content(&env_content)
        .remove("BOT_ID")
        .filter(|id| !id.trim().is_empty());

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
        reused_bot_id,
    })
}

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

    let hub = HubConfig::load_at(&hub_path).map_err(|e| {
        tracing::error!(hub_path = %hub_path.display(), error = %e, "failed to load hub configuration for validation");
        FatalError::Config(format!(
            "failed to load hub configuration at {}: {e}",
            hub_path.display()
        ))
    })?;

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
    let patched_toml =
        residuum_config::patch::apply_patch(&existing_toml, patch_map, "config.toml")
            .map_err(FatalError::Config)?;

    Config::validate_agent_toml(&patched_toml, &agent_path, agent_name, &hub)
        .map_err(FatalError::Config)?;

    let checkpoints = CheckpointEngine::open_for_cli(&hub_path);

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

    if let Err(e) = residuum_core::util::fs::atomic_write(&config_path, &patched_toml).await {
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

    fn teams_template_schema_errors(file: &str, content: &str) -> Vec<String> {
        let filled = content
            .replace("${{TEAMS_APP_ID}}", "0149c02f-e46b-41b3-81be-41b79f31d5b6")
            .replace("${{BOT_ID}}", "b74523f1-6156-4e38-b4d0-f72d908a887d")
            .replace("${{DEVELOPER_URL}}", "https://example.com")
            .replace("${{PRIVACY_URL}}", "https://example.com/privacy")
            .replace("${{TERMS_URL}}", "https://example.com/terms");
        let (schema, doc): (serde_json::Value, serde_json::Value) = match file {
            "m365agents.yml" => (
                serde_json::from_str(include_str!(
                    "../../../../tests/fixtures/m365agents-v1.13.yaml.schema.json"
                ))
                .unwrap(),
                serde_yaml_ng::from_str(content).unwrap(),
            ),
            "appPackage/manifest.json" => (
                serde_json::from_str(include_str!(
                    "../../../../tests/fixtures/teams-manifest-v1.17.schema.json"
                ))
                .unwrap(),
                serde_json::from_str(&filled).unwrap(),
            ),
            other => panic!("no schema for {other}"),
        };
        let validator = jsonschema::validator_for(&schema).unwrap();
        validator
            .iter_errors(&doc)
            .map(|e| format!("{}: {e}", e.instance_path()))
            .collect()
    }

    /// The bundled template with LF line endings, which a Windows checkout turns into CRLF.
    fn bundled_teams_template(file: &str) -> String {
        match file {
            "m365agents.yml" => TEAMS_SETUP_M365AGENTS_YML,
            "appPackage/manifest.json" => TEAMS_SETUP_MANIFEST_JSON,
            other => panic!("no bundled template for {other}"),
        }
        .replace("\r\n", "\n")
    }

    #[test]
    fn the_bundled_teams_templates_match_the_toolkit_schemas() {
        for file in ["m365agents.yml", "appPackage/manifest.json"] {
            assert_eq!(
                teams_template_schema_errors(file, &bundled_teams_template(file)),
                Vec::<String>::new(),
                "{file}"
            );
        }
    }

    const FIXTURE_GLOBAL_CIPHERTEXT: &str = "crypto_42424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242242424242424242424242424242424241c2cf2ab17aa3b23ab952981303de07f9f6d6c37965d8dbdb24bb2f7119299bd5cffe1";
    const FIXTURE_PROJECT_CIPHERTEXT: &str = "crypto_4242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424242424224242424242424242424242424242424d78f4561013ca364f6a6e0e2d2f82d16da4108fc35c7f9d1977bdb3e9f58d2f2a6826b";

    #[test]
    fn decrypt_global_key_fixture() {
        let plain = decrypt_atk_secret(FIXTURE_GLOBAL_CIPHERTEXT, None).unwrap();
        assert_eq!(plain, "TestBotPassword123!");
    }

    #[test]
    fn decrypt_project_fallback_fixture() {
        let err = decrypt_atk_secret(FIXTURE_PROJECT_CIPHERTEXT, None).unwrap_err();
        assert!(matches!(err, AtkDecryptError::DecryptionFailed(_)));

        let err_wrong =
            decrypt_atk_secret(FIXTURE_PROJECT_CIPHERTEXT, Some("wrong-project")).unwrap_err();
        assert!(matches!(err_wrong, AtkDecryptError::DecryptionFailed(_)));

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
        let short = "crypto_42424242";
        let err = decrypt_atk_secret(short, None).unwrap_err();
        assert!(matches!(err, AtkDecryptError::InvalidFormat(_)));
    }

    #[test]
    fn decrypt_tampered_tag_fails() {
        let mut tampered = FIXTURE_GLOBAL_CIPHERTEXT.to_string();
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
        let err = forward_redirect("http://example.com:3000/?code=123")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid redirect host"));

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

        tokio::fs::write(hub_path.join("config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();

        tokio::fs::write(
            agent_path.join("config.toml"),
            "# Initial agent config\n[agent]\nname = \"test-agent\"\n",
        )
        .await
        .unwrap();

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

        let result = import_atk_project(&project_dir, "test-agent", &residuum_root)
            .await
            .unwrap();

        assert_eq!(result.bot_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(result.tenant_id, "tenant-guid-777");
        assert_eq!(result.teams_app_id.as_deref(), Some("teams-app-888"));

        let store = SecretStore::load(&hub_path).unwrap();
        assert_eq!(store.get("teams"), Some("TestBotPassword123!"));

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
        validate_png_dimensions(TEAMS_SETUP_COLOR_PNG, 192, 192, "color icon").unwrap();
        validate_png_dimensions(TEAMS_SETUP_OUTLINE_PNG, 32, 32, "outline icon").unwrap();

        let err_dim =
            validate_png_dimensions(TEAMS_SETUP_COLOR_PNG, 32, 32, "color icon").unwrap_err();
        assert!(
            err_dim
                .to_string()
                .contains("dimensions must be 32x32, found 192x192")
        );

        let err_hdr =
            validate_png_dimensions(b"not a png image at all", 192, 192, "bad icon").unwrap_err();
        assert!(err_hdr.to_string().contains("invalid header"));

        let err_short =
            validate_png_dimensions(b"\x89PNG\r\n\x1a\n", 192, 192, "short icon").unwrap_err();
        assert!(err_short.to_string().contains("invalid header"));
    }

    #[tokio::test]
    async fn scaffold_atk_project_basic_and_rescaffold() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let agent_path = residuum_root.join("scout");
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        let options = AtkScaffoldOptions {
            agent_name: "scout".to_string(),
            endpoint: "https://relay.example.com/teams/inst1/scout".to_string(),
            project_dir: None,
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

        let res = scaffold_atk_project(&residuum_root, &options)
            .await
            .unwrap();
        assert_eq!(res.project_dir, agent_path.join("teams-app"));
        assert!(res.project_dir.join("m365agents.yml").exists());
        assert!(res.project_dir.join("appPackage/manifest.json").exists());
        assert!(res.project_dir.join("appPackage/color.png").exists());
        assert!(res.project_dir.join("appPackage/outline.png").exists());
        assert!(res.project_dir.join("env/.env.residuum").exists());

        let env_content = tokio::fs::read_to_string(res.project_dir.join("env/.env.residuum"))
            .await
            .unwrap();
        assert!(env_content.contains("BOT_DISPLAY_NAME=\"Scout Assistant\""));
        assert!(
            env_content.contains("BOT_ENDPOINT=\"https://relay.example.com/teams/inst1/scout\"")
        );
        assert!(env_content.contains("DEVELOPER_NAME=\"Grizzly Endeavors\""));

        let mut renamed = options.clone();
        renamed.bot_name = Some("Updated Scout".to_string());
        let res2 = scaffold_atk_project(&residuum_root, &renamed)
            .await
            .unwrap();
        assert_eq!(res2.project_dir, res.project_dir);
        let env2 = tokio::fs::read_to_string(res2.project_dir.join("env/.env.residuum"))
            .await
            .unwrap();
        assert!(env2.contains("BOT_DISPLAY_NAME=\"Updated Scout\""));
        assert!(!env2.contains("Scout Assistant"));
        assert_eq!(env2.matches("BOT_DISPLAY_NAME=").count(), 1);
    }

    fn rescaffold_options(bot_name: &str, endpoint: &str) -> AtkScaffoldOptions {
        AtkScaffoldOptions {
            agent_name: "scout".to_string(),
            endpoint: endpoint.to_string(),
            project_dir: None,
            bot_name: Some(bot_name.to_string()),
            developer_name: None,
            developer_url: None,
            privacy_url: None,
            terms_url: None,
            short_description: None,
            long_description: None,
            color_icon: None,
            outline_icon: None,
        }
    }

    #[tokio::test]
    async fn rescaffold_keeps_provisioned_env_and_refreshes_templates() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        tokio::fs::create_dir_all(residuum_root.join("scout"))
            .await
            .unwrap();

        let first = scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout", "https://old.example.com/teams"),
        )
        .await
        .unwrap();

        let env_path = first.project_dir.join("env/.env.residuum");
        let scaffolded = tokio::fs::read_to_string(&env_path).await.unwrap();
        tokio::fs::write(
            &env_path,
            format!(
                "{scaffolded}# written by ATK\nBOT_ID=bot-guid-123\nTEAMS_APP_TENANT_ID=tenant-guid-456\nTEAMS_APP_ID=app-guid-789\nBOT_OBJECT_ID=obj-guid-000\n"
            ),
        )
        .await
        .unwrap();
        let user_env_path = first.project_dir.join("env/.env.residuum.user");
        tokio::fs::write(&user_env_path, "SECRET_BOT_PASSWORD=crypto_abc\n")
            .await
            .unwrap();

        let m365_path = first.project_dir.join("m365agents.yml");
        let manifest_path = first.project_dir.join("appPackage/manifest.json");
        let color_path = first.project_dir.join("appPackage/color.png");
        let project_id = "11111111-2222-3333-4444-555555555555";
        tokio::fs::write(
            &m365_path,
            format!("# stale header\nversion: v0.0\nprojectId: {project_id}\n"),
        )
        .await
        .unwrap();
        tokio::fs::write(&manifest_path, "stale").await.unwrap();
        tokio::fs::write(&color_path, b"stale").await.unwrap();

        let second = scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout Two", "https://new.example.com/teams"),
        )
        .await
        .unwrap();

        let env = tokio::fs::read_to_string(second.project_dir.join("env/.env.residuum"))
            .await
            .unwrap();
        let parsed = parse_env_file(&second.project_dir.join("env/.env.residuum")).unwrap();
        for (key, value) in [
            ("BOT_ID", "bot-guid-123"),
            ("TEAMS_APP_TENANT_ID", "tenant-guid-456"),
            ("TEAMS_APP_ID", "app-guid-789"),
            ("BOT_OBJECT_ID", "obj-guid-000"),
            ("BOT_DISPLAY_NAME", "Scout Two"),
            ("TEAMS_APP_NAME", "Scout Two"),
            ("BOT_ENDPOINT", "https://new.example.com/teams"),
            ("TEAMSFX_ENV", "residuum"),
        ] {
            assert_eq!(
                parsed.get(key).map(String::as_str),
                Some(value),
                "{key} in:\n{env}"
            );
        }
        assert!(env.contains("# written by ATK"));
        assert!(!env.contains("old.example.com"));
        assert_eq!(env.matches("BOT_ENDPOINT=").count(), 1);

        assert_eq!(
            tokio::fs::read_to_string(&user_env_path).await.unwrap(),
            "SECRET_BOT_PASSWORD=crypto_abc\n"
        );
        let m365 = tokio::fs::read_to_string(&m365_path).await.unwrap();
        assert_eq!(
            m365,
            format!("{TEAMS_SETUP_M365AGENTS_YML}projectId: {project_id}\n")
        );
        assert_eq!(m365.matches("projectId:").count(), 1);
        assert_eq!(
            read_project_id_from_yaml(&second.project_dir).as_deref(),
            Some(project_id)
        );
        assert_eq!(second.reused_bot_id.as_deref(), Some("bot-guid-123"));
        assert_eq!(
            tokio::fs::read_to_string(&manifest_path).await.unwrap(),
            TEAMS_SETUP_MANIFEST_JSON
        );
        assert_eq!(
            tokio::fs::read(&color_path).await.unwrap(),
            TEAMS_SETUP_COLOR_PNG
        );
    }

    #[test]
    fn merge_scaffold_env_replaces_owned_keys_and_appends_missing_ones() {
        let entries = [("A", "\"new\"".to_string()), ("B", "\"b\"".to_string())];
        let merged = merge_scaffold_env(
            Some("A=\"old\"\nBOT_ID=keep\nA=\"dup\"\n# note\n"),
            &entries,
        );
        assert_eq!(merged, "A=\"new\"\nBOT_ID=keep\n# note\nB=\"b\"\n");

        assert_eq!(merge_scaffold_env(None, &entries), "A=\"new\"\nB=\"b\"\n");
    }

    #[tokio::test]
    async fn first_scaffold_has_no_reused_bot_and_no_project_id() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        tokio::fs::create_dir_all(residuum_root.join("scout"))
            .await
            .unwrap();
        let res = scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout", "https://example.com/teams"),
        )
        .await
        .unwrap();
        assert_eq!(res.reused_bot_id, None);
        assert_eq!(
            tokio::fs::read_to_string(res.project_dir.join("m365agents.yml"))
                .await
                .unwrap(),
            TEAMS_SETUP_M365AGENTS_YML
        );
    }

    #[tokio::test]
    async fn rescaffold_replaces_unparseable_m365agents_yml_with_the_template() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        tokio::fs::create_dir_all(residuum_root.join("scout"))
            .await
            .unwrap();
        let first = scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout", "https://example.com/teams"),
        )
        .await
        .unwrap();

        let m365_path = first.project_dir.join("m365agents.yml");
        tokio::fs::write(&m365_path, "projectId: [unclosed\n  - : :\n")
            .await
            .unwrap();

        scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout", "https://example.com/teams"),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&m365_path).await.unwrap(),
            TEAMS_SETUP_M365AGENTS_YML
        );
    }

    #[tokio::test]
    async fn rescaffold_fails_visibly_when_existing_m365agents_yml_is_unreadable() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        tokio::fs::create_dir_all(residuum_root.join("scout"))
            .await
            .unwrap();
        let first = scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout", "https://example.com/teams"),
        )
        .await
        .unwrap();

        let m365_path = first.project_dir.join("m365agents.yml");
        tokio::fs::remove_file(&m365_path).await.unwrap();
        tokio::fs::create_dir(&m365_path).await.unwrap();

        let err = scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout", "https://example.com/teams"),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("failed to read existing"), "{err}");
        assert!(m365_path.is_dir());
    }

    #[tokio::test]
    async fn rescaffold_fails_visibly_when_existing_env_is_unreadable() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        tokio::fs::create_dir_all(residuum_root.join("scout"))
            .await
            .unwrap();
        let first = scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout", "https://example.com/teams"),
        )
        .await
        .unwrap();

        let env_path = first.project_dir.join("env/.env.residuum");
        let invalid_utf8 = [b'B', b'O', b'T', b'_', b'I', b'D', b'=', 0xff, 0xfe, b'\n'];
        tokio::fs::write(&env_path, invalid_utf8).await.unwrap();

        let err = scaffold_atk_project(
            &residuum_root,
            &rescaffold_options("Scout", "https://example.com/teams"),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("failed to read existing"), "{err}");
        assert_eq!(tokio::fs::read(&env_path).await.unwrap(), invalid_utf8);
    }

    #[tokio::test]
    async fn scaffold_atk_project_custom_icon_validation() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let agent_path = residuum_root.join("scout");
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        let bad_icon = temp.path().join("bad.png");
        tokio::fs::write(&bad_icon, b"not a valid png")
            .await
            .unwrap();

        let options = AtkScaffoldOptions {
            agent_name: "scout".to_string(),
            endpoint: "https://example.com/teams".to_string(),
            project_dir: None,
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

        tokio::fs::write(hub_path.join("config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();
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

        let err = import_atk_project(&project_dir, "scout", &residuum_root)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid")
                || err.to_string().contains("expected a string")
                || err.to_string().contains("data did not match")
        );

        let store = SecretStore::load(&hub_path).unwrap();
        assert_eq!(store.get("teams"), None);
    }
}
