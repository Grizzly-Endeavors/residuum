use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::constants::{
    DEFAULT_HOP_HARD_LIMIT, DEFAULT_HOP_SOFT_LIMIT, DEFAULT_MAX_CONCURRENT_BACKGROUND,
};
use super::types::{CloudConfig, GatewayConfig, TracingConfig};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    pub input: f64,
    /// Output tokens, reasoning included.
    pub output: f64,
    /// Input tokens read from the prompt cache.
    pub cache_read: f64,
    /// Input tokens written to the prompt cache at the default TTL (5 minutes
    /// on Anthropic).
    pub cache_write: f64,
    /// Input tokens written to the prompt cache with a 1-hour TTL, which
    /// Anthropic bills higher than the default TTL.
    pub cache_write_long: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubA2aConfig {
    pub enabled: bool,
    pub port: u16,
    pub public_url: Option<String>,
}

impl Default for HubA2aConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: super::constants::DEFAULT_A2A_PORT,
            public_url: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubBackgroundConfig {
    pub max_concurrent: usize,
    pub hop_soft_limit: u32,
    pub hop_hard_limit: u32,
}

impl Default for HubBackgroundConfig {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT_BACKGROUND,
            hop_soft_limit: DEFAULT_HOP_SOFT_LIMIT,
            hop_hard_limit: DEFAULT_HOP_HARD_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HubPushConfig {
    pub contact: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemOneProvider {
    TypeSafe,
    Ollama,
    Other,
}

impl SystemOneProvider {
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::TypeSafe => "TypeSafe",
            Self::Ollama => "Ollama",
            Self::Other => "the decision model service",
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TypeSafe => "typesafe",
            Self::Ollama => "ollama",
            Self::Other => "other",
        }
    }

    #[must_use]
    pub fn default_url(self) -> Option<&'static str> {
        match self {
            Self::TypeSafe => Some(super::constants::DEFAULT_TYPESAFE_URL),
            Self::Ollama => Some(super::constants::DEFAULT_OLLAMA_URL),
            Self::Other => None,
        }
    }

    #[must_use]
    pub fn default_model(self) -> Option<&'static str> {
        match self {
            Self::TypeSafe => Some(super::constants::DEFAULT_TYPESAFE_MODEL),
            Self::Ollama | Self::Other => None,
        }
    }
}

impl std::str::FromStr for SystemOneProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "typesafe" => Ok(Self::TypeSafe),
            "ollama" => Ok(Self::Ollama),
            "other" => Ok(Self::Other),
            other => Err(format!(
                "unknown provider \"{other}\", expected typesafe, ollama, or other"
            )),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SystemOneConfig {
    pub provider: SystemOneProvider,
    pub url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub keep_alive: Option<String>,
}

impl SystemOneConfig {
    #[must_use]
    pub fn display_name(&self) -> String {
        if self.provider == SystemOneProvider::Other {
            url::Url::parse(&self.url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| self.provider.display_name().to_string())
        } else {
            self.provider.display_name().to_string()
        }
    }
}

impl std::fmt::Debug for SystemOneConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemOneConfig")
            .field("provider", &self.provider)
            .field("url", &self.url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("keep_alive", &self.keep_alive)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HubConfig {
    pub timezone: chrono_tz::Tz,
    pub gateway: GatewayConfig,
    pub cloud: Option<CloudConfig>,
    pub a2a: HubA2aConfig,
    pub tracing: TracingConfig,
    pub background: HubBackgroundConfig,
    pub push: HubPushConfig,
    pub system_one: Option<SystemOneConfig>,
    pub pricing: BTreeMap<String, ModelPrice>,
    pub config_dir: PathBuf,
    pub load_notices: Vec<String>,
}
