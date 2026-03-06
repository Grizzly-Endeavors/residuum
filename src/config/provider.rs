//! Provider and model specification types.

use std::fmt;
use std::str::FromStr;

use super::constants::{
    DEFAULT_ANTHROPIC_URL, DEFAULT_GEMINI_URL, DEFAULT_OLLAMA_URL, DEFAULT_OPENAI_URL,
};

/// Resolved provider configuration for a specific role.
///
/// Every role (main, observer, reflector, pulse) gets a fully resolved
/// `ProviderSpec` at config load time — no `Option` chains at use sites.
#[derive(Clone, PartialEq)]
pub struct ProviderSpec {
    /// Human-readable identifier (provider entry name or implicit kind).
    pub name: String,
    /// Model spec for this role.
    pub model: ModelSpec,
    /// Base URL for the provider.
    pub provider_url: String,
    /// API key (redacted in Debug output).
    pub api_key: Option<String>,
    /// Ollama `keep_alive` duration (e.g. `"5m"`, `"0"` to unload immediately).
    pub keep_alive: Option<String>,
}

impl fmt::Debug for ProviderSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderSpec")
            .field("name", &self.name)
            .field("model", &self.model)
            .field("provider_url", &self.provider_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("keep_alive", &self.keep_alive)
            .finish()
    }
}

/// Parsed model specification from `"provider/model"` format.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSpec {
    /// The provider kind.
    pub kind: ProviderKind,
    /// The model name (e.g. `"claude-sonnet-4-5"`).
    pub model: String,
}

impl fmt::Display for ModelSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.kind, self.model)
    }
}

impl FromStr for ModelSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider_str, model) = s
            .split_once('/')
            .ok_or_else(|| format!("expected 'provider/model' format, got '{s}'"))?;

        let kind = ProviderKind::from_str(provider_str)?;

        if model.is_empty() {
            return Err("model name cannot be empty".to_string());
        }

        Ok(Self {
            kind,
            model: model.to_string(),
        })
    }
}

/// Supported model provider backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// Anthropic Messages API.
    Anthropic,
    /// Google Gemini `generateContent` API.
    Gemini,
    /// Ollama local inference.
    Ollama,
    /// OpenAI-compatible chat completions (also vLLM, LM Studio, etc.).
    OpenAi,
}

impl ProviderKind {
    /// Default base URL for this provider.
    #[must_use]
    pub(crate) fn default_url(self) -> &'static str {
        match self {
            Self::Anthropic => DEFAULT_ANTHROPIC_URL,
            Self::Gemini => DEFAULT_GEMINI_URL,
            Self::Ollama => DEFAULT_OLLAMA_URL,
            Self::OpenAi => DEFAULT_OPENAI_URL,
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::Gemini => write!(f, "gemini"),
            Self::Ollama => write!(f, "ollama"),
            Self::OpenAi => write!(f, "openai"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "gemini" => Ok(Self::Gemini),
            "ollama" => Ok(Self::Ollama),
            "openai" => Ok(Self::OpenAi),
            other => Err(format!(
                "unknown provider '{other}', expected one of: anthropic, gemini, ollama, openai"
            )),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::super::constants::{
        DEFAULT_ANTHROPIC_URL, DEFAULT_GEMINI_URL, DEFAULT_OLLAMA_URL, DEFAULT_OPENAI_URL,
    };
    use super::*;

    #[test]
    fn model_spec_parse_valid() {
        let spec = ModelSpec::from_str("anthropic/claude-sonnet-4-5").unwrap();
        assert_eq!(spec.kind, ProviderKind::Anthropic, "provider should parse");
        assert_eq!(spec.model, "claude-sonnet-4-5", "model name should parse");
    }

    #[test]
    fn model_spec_parse_ollama() {
        let spec = ModelSpec::from_str("ollama/llama3").unwrap();
        assert_eq!(spec.kind, ProviderKind::Ollama, "ollama should parse");
        assert_eq!(spec.model, "llama3", "model should parse");
    }

    #[test]
    fn model_spec_parse_openai() {
        let spec = ModelSpec::from_str("openai/gpt-4o").unwrap();
        assert_eq!(spec.kind, ProviderKind::OpenAi, "openai should parse");
        assert_eq!(spec.model, "gpt-4o", "model should parse");
    }

    #[test]
    fn model_spec_parse_no_slash() {
        let result = ModelSpec::from_str("just-a-model");
        assert!(result.is_err(), "should require provider/model format");
    }

    #[test]
    fn model_spec_parse_empty_model() {
        let result = ModelSpec::from_str("anthropic/");
        assert!(result.is_err(), "should reject empty model name");
    }

    #[test]
    fn model_spec_parse_unknown_provider() {
        let result = ModelSpec::from_str("unknown/model");
        assert!(result.is_err(), "should reject unknown provider");
    }

    #[test]
    fn model_spec_display() {
        let spec = ModelSpec {
            kind: ProviderKind::Anthropic,
            model: "claude-sonnet-4-5".to_string(),
        };
        assert_eq!(
            spec.to_string(),
            "anthropic/claude-sonnet-4-5",
            "display should round-trip"
        );
    }

    #[test]
    fn model_spec_parse_gemini() {
        let spec = ModelSpec::from_str("gemini/gemini-2.0-flash").unwrap();
        assert_eq!(spec.kind, ProviderKind::Gemini, "gemini should parse");
        assert_eq!(spec.model, "gemini-2.0-flash", "model should parse");
    }

    #[test]
    fn provider_kind_case_insensitive() {
        assert_eq!(
            ProviderKind::from_str("Anthropic").unwrap(),
            ProviderKind::Anthropic,
            "should be case-insensitive"
        );
        assert_eq!(
            ProviderKind::from_str("OLLAMA").unwrap(),
            ProviderKind::Ollama,
            "should be case-insensitive"
        );
    }

    #[test]
    fn config_defaults_provider_kind() {
        assert_eq!(
            ProviderKind::Anthropic.default_url(),
            DEFAULT_ANTHROPIC_URL,
            "anthropic default URL"
        );
        assert_eq!(
            ProviderKind::Ollama.default_url(),
            DEFAULT_OLLAMA_URL,
            "ollama default URL"
        );
        assert_eq!(
            ProviderKind::OpenAi.default_url(),
            DEFAULT_OPENAI_URL,
            "openai default URL"
        );
        assert_eq!(
            ProviderKind::Gemini.default_url(),
            DEFAULT_GEMINI_URL,
            "gemini default URL"
        );
    }
}
