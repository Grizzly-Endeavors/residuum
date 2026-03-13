//! Terminal setup wizard for first-time configuration.

use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use super::provider::ProviderKind;
use crate::error::ResiduumError;

/// Answers collected from the setup wizard (interactive or flags).
#[derive(Debug)]
pub struct WizardAnswers {
    /// IANA timezone (e.g. `"America/New_York"`).
    pub timezone: String,
    /// Selected provider kind.
    pub provider: ProviderKind,
    /// API key (None for Ollama or if user prefers env vars).
    pub api_key: Option<String>,
    /// Model name (e.g. `"claude-sonnet-4-6"`).
    pub model: String,
    /// Standalone web search backend name ("brave", "tavily", or "ollama").
    pub web_search_backend: Option<String>,
    /// API key for the standalone web search backend.
    pub web_search_api_key: Option<String>,
    /// Base URL for Ollama Cloud web search backend.
    pub web_search_base_url: Option<String>,
}

/// Run the interactive terminal wizard.
///
/// Prompts the user for timezone, provider, API key, and model. Returns
/// the collected answers for config generation.
///
/// # Errors
/// Returns `ResiduumError::Config` if stdin/stdout interaction fails or
/// input validation fails.
pub fn run_interactive() -> Result<WizardAnswers, ResiduumError> {
    eprintln!("residuum setup");
    eprintln!("==============");
    eprintln!();

    // 1. Timezone
    let system_tz = iana_time_zone::get_timezone().unwrap_or_default();
    let default_tz = if chrono_tz::Tz::from_str(&system_tz).is_ok() {
        system_tz
    } else {
        String::from("UTC")
    };
    let timezone = loop {
        let input = prompt_with_default(&format!("timezone [{default_tz}]"), &default_tz)?;
        if chrono_tz::Tz::from_str(&input).is_ok() {
            break input;
        }
        eprintln!("  invalid timezone, please enter an IANA timezone (e.g. America/New_York)");
    };

    // 2. Provider
    eprintln!();
    eprintln!("  providers:");
    eprintln!("    1. anthropic");
    eprintln!("    2. openai");
    eprintln!("    3. ollama");
    eprintln!("    4. gemini");
    let provider = loop {
        let input = prompt_with_default("provider [1]", "1")?;
        match input.as_str() {
            "1" | "anthropic" => break ProviderKind::Anthropic,
            "2" | "openai" => break ProviderKind::OpenAi,
            "3" | "ollama" => break ProviderKind::Ollama,
            "4" | "gemini" => break ProviderKind::Gemini,
            other => {
                if let Ok(kind) = ProviderKind::from_str(other) {
                    break kind;
                }
                eprintln!("  invalid choice, enter 1-4 or a provider name");
            }
        }
    };

    // 3. API key (skip for Ollama)
    let api_key = if provider == ProviderKind::Ollama {
        eprintln!();
        eprintln!("  ollama runs locally, no API key needed");
        None
    } else {
        eprintln!();
        eprint!("  api key (press enter to skip, set via env var later): ");
        std::io::stderr().flush().ok();
        let key = rpassword::read_password()
            .map_err(|e| ResiduumError::Config(format!("failed to read api key: {e}")))?;
        if key.trim().is_empty() {
            None
        } else {
            Some(key.trim().to_string())
        }
    };

    // 4. Model
    let default_model = default_model_for_provider(provider);
    eprintln!();
    let model = prompt_with_default(&format!("model [{default_model}]"), default_model)?;

    // 5. Web search (optional)
    let ws = prompt_web_search(provider)?;

    eprintln!();
    Ok(WizardAnswers {
        timezone,
        provider,
        api_key,
        model,
        web_search_backend: ws.backend,
        web_search_api_key: ws.api_key,
        web_search_base_url: ws.base_url,
    })
}

/// Build answers from CLI flags (non-interactive mode).
///
/// # Errors
/// Returns `ResiduumError::Config` if required fields are missing or
/// timezone validation fails.
pub fn from_flags(
    timezone: Option<&str>,
    provider: Option<&str>,
    api_key: Option<&str>,
    model: Option<&str>,
    web_search_backend: Option<&str>,
    web_search_api_key: Option<&str>,
    web_search_base_url: Option<&str>,
) -> Result<WizardAnswers, ResiduumError> {
    let timezone = timezone.ok_or_else(|| {
        ResiduumError::Config("--timezone is required in non-interactive mode".to_string())
    })?;

    // Validate timezone
    chrono_tz::Tz::from_str(timezone)
        .map_err(|err| ResiduumError::Config(format!("invalid timezone '{timezone}': {err}")))?;

    let provider_str = provider.ok_or_else(|| {
        ResiduumError::Config("--provider is required in non-interactive mode".to_string())
    })?;
    let provider_kind = ProviderKind::from_str(provider_str).map_err(ResiduumError::Config)?;

    let default_model = default_model_for_provider(provider_kind);
    let model = model.unwrap_or(default_model).to_string();

    // Validate web search backend if provided
    let web_search_backend = if let Some(backend) = web_search_backend {
        match backend {
            "brave" | "tavily" | "ollama" => {}
            other => {
                return Err(ResiduumError::Config(format!(
                    "invalid web search backend '{other}': must be brave, tavily, or ollama"
                )));
            }
        }
        if (backend == "brave" || backend == "tavily") && web_search_api_key.is_none() {
            tracing::warn!(
                backend,
                "--web-search-api-key not provided; web search may not work without it"
            );
        }
        Some(backend.to_string())
    } else {
        None
    };

    Ok(WizardAnswers {
        timezone: timezone.to_string(),
        provider: provider_kind,
        api_key: api_key.map(ToString::to_string),
        model,
        web_search_backend,
        web_search_api_key: web_search_api_key.map(ToString::to_string),
        web_search_base_url: web_search_base_url.map(ToString::to_string),
    })
}

/// Write config.toml and providers.toml from wizard answers.
///
/// # Errors
/// Returns `ResiduumError::Config` if writing either file fails.
pub fn write_config(dir: &Path, answers: &WizardAnswers) -> Result<(), ResiduumError> {
    // config.toml — timezone only
    let config_path = dir.join("config.toml");
    let mut config_lines = Vec::new();
    config_lines.push("# Residuum configuration — generated by setup wizard".to_string());
    config_lines.push(String::new());
    config_lines.push(format!("timezone = \"{}\"", answers.timezone));
    config_lines.push(String::new());

    // Append web search section if configured
    if let Some(ref backend) = answers.web_search_backend {
        config_lines.push("[web_search]".to_string());
        config_lines.push(format!("backend = \"{backend}\""));
        config_lines.push(String::new());

        config_lines.push(format!("[web_search.{backend}]"));
        if let Some(ref key) = answers.web_search_api_key {
            config_lines.push(format!("api_key = \"{key}\""));
        }
        if let Some(ref url) = answers.web_search_base_url {
            config_lines.push(format!("base_url = \"{url}\""));
        }
        config_lines.push(String::new());
    }

    let config_content = config_lines.join("\n");
    std::fs::write(&config_path, &config_content).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to write config.toml at {}: {e}",
            config_path.display()
        ))
    })?;

    // providers.toml — models + optional provider
    let providers_path = dir.join("providers.toml");
    let mut prov_lines = Vec::new();
    prov_lines.push("# Provider configuration — generated by setup wizard".to_string());
    prov_lines.push(String::new());
    prov_lines.push("[models]".to_string());
    prov_lines.push(format!("main = \"{}/{}\"", answers.provider, answers.model));

    if let Some(ref key) = answers.api_key {
        prov_lines.push(String::new());
        prov_lines.push(format!("[providers.{}]", answers.provider));
        prov_lines.push(format!("type = \"{}\"", answers.provider));
        prov_lines.push(format!("api_key = \"{key}\""));
    }

    prov_lines.push(String::new());

    let prov_content = prov_lines.join("\n");
    std::fs::write(&providers_path, &prov_content).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to write providers.toml at {}: {e}",
            providers_path.display()
        ))
    })?;

    Ok(())
}

/// Web search wizard answers (backend, api key, base url).
struct WebSearchWizardAnswers {
    backend: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
}

/// Prompt the user for optional web search backend configuration.
fn prompt_web_search(provider: ProviderKind) -> Result<WebSearchWizardAnswers, ResiduumError> {
    eprintln!();
    eprintln!("  step 5: web search (optional)");
    eprintln!();
    eprintln!("  web search lets the agent look up real-time information.");

    let has_native_search = matches!(
        provider,
        ProviderKind::Anthropic | ProviderKind::OpenAi | ProviderKind::Gemini
    );

    let configure_standalone = if has_native_search {
        eprintln!("  provider-native search is automatically enabled for {provider}.");
        eprintln!();
        let answer =
            prompt_with_default("configure a standalone web search backend too? [y/N]", "n")?;
        answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
    } else {
        eprintln!("  {provider} does not support native web search.");
        eprintln!();
        let answer = prompt_with_default("configure a web search backend? [y/N]", "n")?;
        answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
    };

    if !configure_standalone {
        return Ok(WebSearchWizardAnswers {
            backend: None,
            api_key: None,
            base_url: None,
        });
    }

    eprintln!();
    eprintln!("  backends:");
    eprintln!("    1. brave search");
    eprintln!("    2. tavily");
    eprintln!("    3. ollama cloud");
    let backend = loop {
        let input = prompt_with_default("backend [1]", "1")?;
        match input.as_str() {
            "1" | "brave" => break "brave".to_string(),
            "2" | "tavily" => break "tavily".to_string(),
            "3" | "ollama" => break "ollama".to_string(),
            _ => eprintln!("  invalid choice, enter 1-3 or a backend name"),
        }
    };

    eprintln!();
    eprint!("  web search api key: ");
    std::io::stderr().flush().ok();
    let ws_key = rpassword::read_password()
        .map_err(|e| ResiduumError::Config(format!("failed to read web search api key: {e}")))?;
    let ws_key = if ws_key.trim().is_empty() {
        None
    } else {
        Some(ws_key.trim().to_string())
    };

    let ws_base_url = if backend == "ollama" {
        eprintln!();
        let url = prompt_with_default(
            "base url [https://api.ollama.com]",
            "https://api.ollama.com",
        )?;
        Some(url)
    } else {
        None
    };

    Ok(WebSearchWizardAnswers {
        backend: Some(backend),
        api_key: ws_key,
        base_url: ws_base_url,
    })
}

/// Default model name for a given provider.
#[must_use]
fn default_model_for_provider(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "claude-sonnet-4-6",
        ProviderKind::OpenAi => "gpt-4o",
        ProviderKind::Ollama => "llama3",
        ProviderKind::Gemini => "gemini-2.0-flash",
    }
}

/// Prompt the user with a default value, returning the default on empty input.
fn prompt_with_default(prompt: &str, default: &str) -> Result<String, ResiduumError> {
    eprint!("  {prompt}: ");
    std::io::stderr().flush().ok();

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| ResiduumError::Config(format!("failed to read input: {e}")))?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn from_flags_all_present() {
        let answers = from_flags(
            Some("UTC"),
            Some("anthropic"),
            Some("sk-test"),
            Some("claude-sonnet-4-6"),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(answers.timezone, "UTC", "timezone should match");
        assert_eq!(
            answers.provider,
            ProviderKind::Anthropic,
            "provider should match"
        );
        assert_eq!(
            answers.api_key.as_deref(),
            Some("sk-test"),
            "api key should match"
        );
        assert_eq!(answers.model, "claude-sonnet-4-6", "model should match");
    }

    #[test]
    fn from_flags_missing_timezone() {
        let result = from_flags(None, Some("anthropic"), None, None, None, None, None);
        assert!(result.is_err(), "should require timezone");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timezone"),
            "error should mention timezone: {err}"
        );
    }

    #[test]
    fn from_flags_missing_provider() {
        let result = from_flags(Some("UTC"), None, None, None, None, None, None);
        assert!(result.is_err(), "should require provider");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("provider"),
            "error should mention provider: {err}"
        );
    }

    #[test]
    fn from_flags_default_model_ollama() {
        let answers =
            from_flags(Some("UTC"), Some("ollama"), None, None, None, None, None).unwrap();
        assert_eq!(answers.model, "llama3", "ollama should default to llama3");
    }

    #[test]
    fn from_flags_default_model_openai() {
        let answers =
            from_flags(Some("UTC"), Some("openai"), None, None, None, None, None).unwrap();
        assert_eq!(answers.model, "gpt-4o", "openai should default to gpt-4o");
    }

    #[test]
    fn from_flags_default_model_gemini() {
        let answers =
            from_flags(Some("UTC"), Some("gemini"), None, None, None, None, None).unwrap();
        assert_eq!(
            answers.model, "gemini-2.0-flash",
            "gemini should default to gemini-2.0-flash"
        );
    }

    #[test]
    fn from_flags_invalid_timezone() {
        let result = from_flags(
            Some("Not/A/Timezone"),
            Some("anthropic"),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(result.is_err(), "should reject invalid timezone");
    }

    #[test]
    fn from_flags_invalid_provider() {
        let result = from_flags(Some("UTC"), Some("notreal"), None, None, None, None, None);
        assert!(result.is_err(), "should reject invalid provider");
    }

    #[test]
    fn write_config_basic() {
        let dir = tempfile::tempdir().unwrap();
        let answers = WizardAnswers {
            timezone: "America/New_York".to_string(),
            provider: ProviderKind::Anthropic,
            api_key: Some("sk-test-key".to_string()),
            model: "claude-sonnet-4-6".to_string(),
            web_search_backend: None,
            web_search_api_key: None,
            web_search_base_url: None,
        };

        write_config(dir.path(), &answers).unwrap();

        // config.toml — timezone only
        let config = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            config.contains("timezone = \"America/New_York\""),
            "config.toml should contain timezone: {config}"
        );
        assert!(
            !config.contains("[models]"),
            "config.toml should not contain [models]: {config}"
        );

        // providers.toml — models + provider
        let providers = std::fs::read_to_string(dir.path().join("providers.toml")).unwrap();
        assert!(
            providers.contains("main = \"anthropic/claude-sonnet-4-6\""),
            "providers.toml should contain model spec: {providers}"
        );
        assert!(
            providers.contains("[providers.anthropic]"),
            "providers.toml should contain provider section: {providers}"
        );
        assert!(
            providers.contains("api_key = \"sk-test-key\""),
            "providers.toml should contain api key: {providers}"
        );
    }

    #[test]
    fn write_config_no_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let answers = WizardAnswers {
            timezone: "UTC".to_string(),
            provider: ProviderKind::Ollama,
            api_key: None,
            model: "llama3".to_string(),
            web_search_backend: None,
            web_search_api_key: None,
            web_search_base_url: None,
        };

        write_config(dir.path(), &answers).unwrap();

        // config.toml — timezone only
        let config = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            config.contains("timezone = \"UTC\""),
            "config.toml should contain timezone: {config}"
        );

        // providers.toml — models, no provider section
        let providers = std::fs::read_to_string(dir.path().join("providers.toml")).unwrap();
        assert!(
            providers.contains("main = \"ollama/llama3\""),
            "providers.toml should contain model spec: {providers}"
        );
        assert!(
            !providers.contains("[providers"),
            "providers.toml should not have provider section without api key: {providers}"
        );
    }

    #[test]
    fn from_flags_with_web_search() {
        let answers = from_flags(
            Some("UTC"),
            Some("ollama"),
            None,
            None,
            Some("brave"),
            Some("brv-key-123"),
            None,
        )
        .unwrap();

        assert_eq!(
            answers.web_search_backend.as_deref(),
            Some("brave"),
            "web search backend should be brave"
        );
        assert_eq!(
            answers.web_search_api_key.as_deref(),
            Some("brv-key-123"),
            "web search api key should match"
        );
        assert!(
            answers.web_search_base_url.is_none(),
            "base url should be None for brave"
        );
    }

    #[test]
    fn from_flags_with_ollama_web_search() {
        let answers = from_flags(
            Some("UTC"),
            Some("ollama"),
            None,
            None,
            Some("ollama"),
            Some("oll-key"),
            Some("https://custom.ollama.com"),
        )
        .unwrap();

        assert_eq!(
            answers.web_search_backend.as_deref(),
            Some("ollama"),
            "web search backend should be ollama"
        );
        assert_eq!(
            answers.web_search_api_key.as_deref(),
            Some("oll-key"),
            "web search api key should match"
        );
        assert_eq!(
            answers.web_search_base_url.as_deref(),
            Some("https://custom.ollama.com"),
            "base url should match"
        );
    }

    #[test]
    fn from_flags_invalid_web_search_backend() {
        let result = from_flags(
            Some("UTC"),
            Some("anthropic"),
            Some("sk-test"),
            None,
            Some("invalid-backend"),
            None,
            None,
        );
        assert!(result.is_err(), "should reject invalid web search backend");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid web search backend"),
            "error should mention invalid backend: {err}"
        );
    }

    #[test]
    fn write_config_with_brave_web_search() {
        let dir = tempfile::tempdir().unwrap();
        let answers = WizardAnswers {
            timezone: "UTC".to_string(),
            provider: ProviderKind::Anthropic,
            api_key: Some("sk-test".to_string()),
            model: "claude-sonnet-4-6".to_string(),
            web_search_backend: Some("brave".to_string()),
            web_search_api_key: Some("brv-key-abc".to_string()),
            web_search_base_url: None,
        };

        write_config(dir.path(), &answers).unwrap();

        let config = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            config.contains("[web_search]"),
            "config.toml should contain [web_search]: {config}"
        );
        assert!(
            config.contains("backend = \"brave\""),
            "config.toml should contain backend = brave: {config}"
        );
        assert!(
            config.contains("[web_search.brave]"),
            "config.toml should contain [web_search.brave]: {config}"
        );
        assert!(
            config.contains("api_key = \"brv-key-abc\""),
            "config.toml should contain web search api key: {config}"
        );
    }

    #[test]
    fn write_config_with_ollama_web_search() {
        let dir = tempfile::tempdir().unwrap();
        let answers = WizardAnswers {
            timezone: "UTC".to_string(),
            provider: ProviderKind::Ollama,
            api_key: None,
            model: "llama3".to_string(),
            web_search_backend: Some("ollama".to_string()),
            web_search_api_key: Some("oll-key-xyz".to_string()),
            web_search_base_url: Some("https://api.ollama.com".to_string()),
        };

        write_config(dir.path(), &answers).unwrap();

        let config = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            config.contains("[web_search]"),
            "config.toml should contain [web_search]: {config}"
        );
        assert!(
            config.contains("backend = \"ollama\""),
            "config.toml should contain backend = ollama: {config}"
        );
        assert!(
            config.contains("[web_search.ollama]"),
            "config.toml should contain [web_search.ollama]: {config}"
        );
        assert!(
            config.contains("api_key = \"oll-key-xyz\""),
            "config.toml should contain web search api key: {config}"
        );
        assert!(
            config.contains("base_url = \"https://api.ollama.com\""),
            "config.toml should contain base_url: {config}"
        );
    }
}
