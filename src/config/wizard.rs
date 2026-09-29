//! Terminal setup wizard for first-time configuration.

use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use super::paths::validate_agent_name;
use super::provider::ProviderKind;
use crate::util::FatalError;

/// Default name for the first agent when the user accepts the default.
const DEFAULT_AGENT_NAME: &str = "assistant";

/// Answers collected from the setup wizard (interactive or flags).
#[derive(Debug)]
pub struct WizardAnswers {
    /// The user's name, written to `USER.md`. `None` if skipped.
    pub user_name: Option<String>,
    /// The first agent's name (validated with
    /// [`validate_agent_name`]) — its directory name and identity
    /// everywhere.
    pub agent_name: String,
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
/// Prompts the user for their name, the first agent's name, timezone,
/// provider, API key, and model. Returns the collected answers for config
/// generation.
///
/// # Errors
/// Returns `FatalError::Config` if stdin/stdout interaction fails or
/// input validation fails.
pub fn run_interactive() -> Result<WizardAnswers, FatalError> {
    println!("residuum setup");
    println!("==============");
    println!();

    // 0. User's name (optional)
    print!("  what should residuum call you? (optional, press enter to skip): ");
    std::io::stdout().flush().ok();
    let mut name_input = String::new();
    std::io::stdin()
        .read_line(&mut name_input)
        .map_err(|e| FatalError::Config(format!("failed to read input: {e}")))?;
    let user_name = {
        let trimmed = name_input.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };

    // 0b. First agent's name
    println!();
    let agent_name = loop {
        let input = prompt_with_default(
            &format!("agent name [{DEFAULT_AGENT_NAME}]"),
            DEFAULT_AGENT_NAME,
        )?;
        match validate_agent_name(&input) {
            Ok(()) => break input,
            Err(e) => println!("  {e}"),
        }
    };

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
        println!("  invalid timezone, please enter an IANA timezone (e.g. America/New_York)");
    };

    // 2. Provider
    println!();
    println!("  providers:");
    println!("    1. anthropic");
    println!("    2. openai");
    println!("    3. ollama");
    println!("    4. gemini");
    println!("    5. fireworks");
    let provider = loop {
        let input = prompt_with_default("provider [1]", "1")?;
        match input.as_str() {
            "1" | "anthropic" => break ProviderKind::Anthropic,
            "2" | "openai" => break ProviderKind::OpenAi,
            "3" | "ollama" => break ProviderKind::Ollama,
            "4" | "gemini" => break ProviderKind::Gemini,
            "5" | "fireworks" => break ProviderKind::Fireworks,
            other => {
                if let Ok(kind) = ProviderKind::from_str(other) {
                    break kind;
                }
                println!("  invalid choice, enter 1-5 or a provider name");
            }
        }
    };

    // 3. API key (skip for Ollama)
    let api_key = if provider == ProviderKind::Ollama {
        println!();
        println!("  ollama runs locally, no API key needed");
        None
    } else {
        println!();
        print!("  api key (press enter to skip, set via env var later): ");
        std::io::stdout().flush().ok();
        let key = rpassword::read_password()
            .map_err(|e| FatalError::Config(format!("failed to read api key: {e}")))?;
        if key.trim().is_empty() {
            None
        } else {
            Some(key.trim().to_string())
        }
    };

    // 4. Model
    let default_model = default_model_for_provider(provider);
    println!();
    let model = prompt_with_default(&format!("model [{default_model}]"), default_model)?;

    // 5. Web search (optional)
    let ws = prompt_web_search(provider)?;

    println!();
    Ok(WizardAnswers {
        user_name,
        agent_name,
        timezone,
        provider,
        api_key,
        model,
        web_search_backend: ws.backend,
        web_search_api_key: ws.api_key,
        web_search_base_url: ws.base_url,
    })
}

/// CLI flags accepted by the non-interactive setup path. Bundled into one
/// struct (rather than passed as separate parameters) purely to stay under
/// the function-argument-count lint — `commands::setup::SetupArgs` mirrors
/// this shape one-for-one and its fields are passed straight through.
#[derive(Debug, Default)]
pub struct WizardFlags<'a> {
    /// See [`WizardAnswers::user_name`].
    pub user_name: Option<&'a str>,
    /// See [`WizardAnswers::agent_name`]. Defaults to `"assistant"` when unset.
    pub agent_name: Option<&'a str>,
    pub timezone: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub api_key: Option<&'a str>,
    pub model: Option<&'a str>,
    pub web_search_backend: Option<&'a str>,
    pub web_search_api_key: Option<&'a str>,
    pub web_search_base_url: Option<&'a str>,
}

/// Build answers from CLI flags (non-interactive mode).
///
/// # Errors
/// Returns `FatalError::Config` if required fields are missing, the
/// timezone or agent name fails validation, or the web search backend is
/// unrecognized.
pub fn from_flags(flags: &WizardFlags<'_>) -> Result<WizardAnswers, FatalError> {
    let timezone = flags.timezone.ok_or_else(|| {
        FatalError::Config("--timezone is required in non-interactive mode".to_string())
    })?;

    // Validate timezone
    chrono_tz::Tz::from_str(timezone)
        .map_err(|err| FatalError::Config(format!("invalid timezone '{timezone}': {err}")))?;

    let agent_name = flags.agent_name.unwrap_or(DEFAULT_AGENT_NAME);
    validate_agent_name(agent_name).map_err(FatalError::Config)?;

    let provider_str = flags.provider.ok_or_else(|| {
        FatalError::Config("--provider is required in non-interactive mode".to_string())
    })?;
    let provider_kind = ProviderKind::from_str(provider_str).map_err(FatalError::Config)?;

    let default_model = default_model_for_provider(provider_kind);
    let model = flags.model.unwrap_or(default_model).to_string();

    // Validate web search backend if provided
    let web_search_backend = if let Some(backend) = flags.web_search_backend {
        if !matches!(backend, "brave" | "tavily" | "ollama") {
            return Err(FatalError::Config(format!(
                "invalid web search backend '{backend}': must be brave, tavily, or ollama"
            )));
        }
        if (backend == "brave" || backend == "tavily") && flags.web_search_api_key.is_none() {
            tracing::warn!(
                %backend,
                "--web-search-api-key not provided; web search may not work without it"
            );
        }
        Some(backend.to_string())
    } else {
        None
    };

    Ok(WizardAnswers {
        user_name: flags.user_name.map(ToString::to_string),
        agent_name: agent_name.to_string(),
        timezone: timezone.to_string(),
        provider: provider_kind,
        api_key: flags.api_key.map(ToString::to_string),
        model,
        web_search_backend,
        web_search_api_key: flags.web_search_api_key.map(ToString::to_string),
        web_search_base_url: flags.web_search_base_url.map(ToString::to_string),
    })
}

/// Write `hub/config.toml`, bootstrap and write the first agent's full
/// workspace (`SOUL.md`, wiki, bundled skills, `USER.md` personalized with
/// [`WizardAnswers::user_name`]), and write its `config.toml`/
/// `providers.toml` from the wizard answers.
///
/// `residuum_root` is `~/.residuum` (or an override, e.g. for tests/the
/// isolated `--setup` temp-directory flow); the hub lives at
/// `residuum_root/hub` and the agent at `residuum_root/<agent_name>`.
///
/// # Errors
/// Returns `FatalError::Config`/`FatalError::Workspace` if bootstrapping the
/// hub or agent directories, or writing any file, fails.
pub async fn write_config(residuum_root: &Path, answers: &WizardAnswers) -> Result<(), FatalError> {
    let hub_dir = super::paths::hub_dir(residuum_root);
    super::HubConfig::bootstrap_at(&hub_dir)?;

    // hub/config.toml — timezone only
    let hub_config_path = hub_dir.join("config.toml");
    let hub_content = format!(
        "# Hub configuration — generated by setup wizard\n\ntimezone = \"{}\"\n",
        answers.timezone
    );
    crate::util::fs::atomic_write(&hub_config_path, &hub_content)
        .await
        .map_err(|e| {
            FatalError::Config(format!(
                "failed to write hub config.toml at {}: {e:#}",
                hub_config_path.display()
            ))
        })?;

    // The agent's full workspace: identity files, wiki, bundled skills, and
    // USER.md personalized with the user's name.
    let agent_dir = residuum_root.join(&answers.agent_name);
    let layout = crate::workspace::layout::WorkspaceLayout::new(&agent_dir);
    crate::workspace::bootstrap::ensure_workspace(
        &layout,
        answers.user_name.as_deref(),
        Some(&answers.timezone),
    )
    .await
    .map_err(|e| FatalError::Workspace(e.to_string()))?;

    let agent_config_dir = layout.config_dir();
    super::Config::bootstrap_agent_config_dir(&agent_config_dir)?;

    // The wizard sets only web search in the agent's config.toml; every other
    // section takes its default.
    let config_path = agent_config_dir.join("config.toml");
    let mut config_lines = Vec::new();
    config_lines.push("# Agent configuration — generated by setup wizard".to_string());
    config_lines.push(String::new());

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

    // providers.toml — models + optional provider
    let providers_path = agent_config_dir.join("providers.toml");
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
    crate::util::fs::atomic_write(&providers_path, &prov_content)
        .await
        .map_err(|e| {
            FatalError::Config(format!(
                "failed to write providers.toml at {}: {e:#}",
                providers_path.display()
            ))
        })?;

    // config.toml goes last: it is the file that makes the directory
    // discoverable as an agent, so a failure above never leaves a
    // half-configured agent behind.
    crate::util::fs::atomic_write(&config_path, &config_content)
        .await
        .map_err(|e| {
            FatalError::Config(format!(
                "failed to write config.toml at {}: {e:#}",
                config_path.display()
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
fn prompt_web_search(provider: ProviderKind) -> Result<WebSearchWizardAnswers, FatalError> {
    println!();
    println!("  step 5: web search (optional)");
    println!();
    println!("  web search lets the agent look up real-time information.");

    let has_native_search = matches!(
        provider,
        ProviderKind::Anthropic | ProviderKind::OpenAi | ProviderKind::Gemini
    );

    let configure_standalone = if has_native_search {
        println!("  provider-native search is automatically enabled for {provider}.");
        println!();
        let answer =
            prompt_with_default("configure a standalone web search backend too? [y/N]", "n")?;
        answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
    } else {
        println!("  {provider} does not support native web search.");
        println!();
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

    println!();
    println!("  backends:");
    println!("    1. brave search");
    println!("    2. tavily");
    println!("    3. ollama cloud");
    let backend = loop {
        let input = prompt_with_default("backend [1]", "1")?;
        match input.as_str() {
            "1" | "brave" => break "brave".to_string(),
            "2" | "tavily" => break "tavily".to_string(),
            "3" | "ollama" => break "ollama".to_string(),
            _ => println!("  invalid choice, enter 1-3 or a backend name"),
        }
    };

    println!();
    print!("  web search api key: ");
    std::io::stdout().flush().ok();
    let ws_key = rpassword::read_password()
        .map_err(|e| FatalError::Config(format!("failed to read web search api key: {e}")))?;
    let ws_key = if ws_key.trim().is_empty() {
        None
    } else {
        Some(ws_key.trim().to_string())
    };

    let ws_base_url = if backend == "ollama" {
        println!();
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
        // A router tracks Fireworks' current model; fixed model ids get retired.
        ProviderKind::Fireworks => "accounts/fireworks/routers/glm-flash-latest",
    }
}

/// Prompt the user with a default value, returning the default on empty input.
fn prompt_with_default(prompt: &str, default: &str) -> Result<String, FatalError> {
    print!("  {prompt}: ");
    std::io::stdout().flush().ok();

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| FatalError::Config(format!("failed to read input: {e}")))?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags<'a>(timezone: Option<&'a str>, provider: Option<&'a str>) -> WizardFlags<'a> {
        WizardFlags {
            timezone,
            provider,
            ..WizardFlags::default()
        }
    }

    #[test]
    fn from_flags_all_present() {
        let answers = from_flags(&WizardFlags {
            timezone: Some("UTC"),
            provider: Some("anthropic"),
            api_key: Some("sk-test"),
            model: Some("claude-sonnet-4-6"),
            agent_name: Some("my-agent"),
            user_name: Some("Alex"),
            ..WizardFlags::default()
        })
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
        assert_eq!(answers.agent_name, "my-agent", "agent name should match");
        assert_eq!(
            answers.user_name.as_deref(),
            Some("Alex"),
            "user name should match"
        );
    }

    #[test]
    fn from_flags_agent_name_defaults() {
        let answers = from_flags(&flags(Some("UTC"), Some("ollama"))).unwrap();
        assert_eq!(answers.agent_name, DEFAULT_AGENT_NAME);
        assert!(answers.user_name.is_none());
    }

    #[test]
    fn from_flags_invalid_agent_name_is_rejected() {
        let result = from_flags(&WizardFlags {
            agent_name: Some("Not Valid!"),
            ..flags(Some("UTC"), Some("anthropic"))
        });
        assert!(result.is_err(), "should reject an invalid agent name");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("agent name"), "{err}");
    }

    #[test]
    fn from_flags_missing_timezone() {
        let result = from_flags(&flags(None, Some("anthropic")));
        assert!(result.is_err(), "should require timezone");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timezone"),
            "error should mention timezone: {err}"
        );
    }

    #[test]
    fn from_flags_missing_provider() {
        let result = from_flags(&flags(Some("UTC"), None));
        assert!(result.is_err(), "should require provider");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("provider"),
            "error should mention provider: {err}"
        );
    }

    #[test]
    fn from_flags_default_model_ollama() {
        let answers = from_flags(&flags(Some("UTC"), Some("ollama"))).unwrap();
        assert_eq!(answers.model, "llama3", "ollama should default to llama3");
    }

    #[test]
    fn from_flags_default_model_openai() {
        let answers = from_flags(&flags(Some("UTC"), Some("openai"))).unwrap();
        assert_eq!(answers.model, "gpt-4o", "openai should default to gpt-4o");
    }

    #[test]
    fn from_flags_default_model_gemini() {
        let answers = from_flags(&flags(Some("UTC"), Some("gemini"))).unwrap();
        assert_eq!(
            answers.model, "gemini-2.0-flash",
            "gemini should default to gemini-2.0-flash"
        );
    }

    #[test]
    fn from_flags_default_model_anthropic() {
        let answers = from_flags(&flags(Some("UTC"), Some("anthropic"))).unwrap();
        assert_eq!(
            answers.model, "claude-sonnet-4-6",
            "anthropic should default to claude-sonnet-4-6"
        );
    }

    #[test]
    fn from_flags_invalid_timezone() {
        let result = from_flags(&flags(Some("Not/A/Timezone"), Some("anthropic")));
        assert!(result.is_err(), "should reject invalid timezone");
    }

    #[test]
    fn from_flags_invalid_provider() {
        let result = from_flags(&flags(Some("UTC"), Some("notreal")));
        assert!(result.is_err(), "should reject invalid provider");
    }

    fn test_answers(
        agent_name: &str,
        timezone: &str,
        provider: ProviderKind,
        api_key: Option<&str>,
        model: &str,
    ) -> WizardAnswers {
        WizardAnswers {
            user_name: None,
            agent_name: agent_name.to_string(),
            timezone: timezone.to_string(),
            provider,
            api_key: api_key.map(ToString::to_string),
            model: model.to_string(),
            web_search_backend: None,
            web_search_api_key: None,
            web_search_base_url: None,
        }
    }

    #[tokio::test]
    async fn write_config_basic() {
        let dir = tempfile::tempdir().unwrap();
        let answers = test_answers(
            "assistant",
            "America/New_York",
            ProviderKind::Anthropic,
            Some("sk-test-key"),
            "claude-sonnet-4-6",
        );

        write_config(dir.path(), &answers).await.unwrap();

        // hub/config.toml — timezone only
        let hub_config =
            std::fs::read_to_string(dir.path().join("hub").join("config.toml")).unwrap();
        assert!(
            hub_config.contains("timezone = \"America/New_York\""),
            "hub config.toml should contain timezone: {hub_config}"
        );
        assert!(
            !hub_config.contains("[models]"),
            "hub config.toml should not contain [models]: {hub_config}"
        );

        let agent_config_dir = dir.path().join("assistant").join("config");
        let config = std::fs::read_to_string(agent_config_dir.join("config.toml")).unwrap();
        assert!(
            !config.contains("[models]"),
            "agent config.toml should not contain [models]: {config}"
        );

        // providers.toml — models + provider
        let providers = std::fs::read_to_string(agent_config_dir.join("providers.toml")).unwrap();
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

        // The agent's workspace was bootstrapped too.
        assert!(
            dir.path().join("assistant").join("SOUL.md").exists(),
            "SOUL.md should be written as part of the agent's workspace bootstrap"
        );
    }

    #[tokio::test]
    async fn write_config_no_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let answers = test_answers("assistant", "UTC", ProviderKind::Ollama, None, "llama3");

        write_config(dir.path(), &answers).await.unwrap();

        let hub_config =
            std::fs::read_to_string(dir.path().join("hub").join("config.toml")).unwrap();
        assert!(
            hub_config.contains("timezone = \"UTC\""),
            "hub config.toml should contain timezone: {hub_config}"
        );

        // providers.toml — models, no provider section
        let providers = std::fs::read_to_string(
            dir.path()
                .join("assistant")
                .join("config")
                .join("providers.toml"),
        )
        .unwrap();
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
        let answers = from_flags(&WizardFlags {
            web_search_backend: Some("brave"),
            web_search_api_key: Some("brv-key-123"),
            ..flags(Some("UTC"), Some("ollama"))
        })
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
        let answers = from_flags(&WizardFlags {
            web_search_backend: Some("ollama"),
            web_search_api_key: Some("oll-key"),
            web_search_base_url: Some("https://custom.ollama.com"),
            ..flags(Some("UTC"), Some("ollama"))
        })
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
        let result = from_flags(&WizardFlags {
            api_key: Some("sk-test"),
            web_search_backend: Some("invalid-backend"),
            ..flags(Some("UTC"), Some("anthropic"))
        });
        assert!(result.is_err(), "should reject invalid web search backend");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid web search backend"),
            "error should mention invalid backend: {err}"
        );
    }

    #[tokio::test]
    async fn write_config_with_brave_web_search() {
        let dir = tempfile::tempdir().unwrap();
        let mut answers = test_answers(
            "assistant",
            "UTC",
            ProviderKind::Anthropic,
            Some("sk-test"),
            "claude-sonnet-4-6",
        );
        answers.web_search_backend = Some("brave".to_string());
        answers.web_search_api_key = Some("brv-key-abc".to_string());

        write_config(dir.path(), &answers).await.unwrap();

        let config = std::fs::read_to_string(
            dir.path()
                .join("assistant")
                .join("config")
                .join("config.toml"),
        )
        .unwrap();
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

    #[tokio::test]
    async fn write_config_with_ollama_web_search() {
        let dir = tempfile::tempdir().unwrap();
        let mut answers = test_answers("assistant", "UTC", ProviderKind::Ollama, None, "llama3");
        answers.web_search_backend = Some("ollama".to_string());
        answers.web_search_api_key = Some("oll-key-xyz".to_string());
        answers.web_search_base_url = Some("https://api.ollama.com".to_string());

        write_config(dir.path(), &answers).await.unwrap();

        let config = std::fs::read_to_string(
            dir.path()
                .join("assistant")
                .join("config")
                .join("config.toml"),
        )
        .unwrap();
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

    #[tokio::test]
    async fn write_config_personalizes_user_md() {
        let dir = tempfile::tempdir().unwrap();
        let mut answers = test_answers("assistant", "UTC", ProviderKind::Ollama, None, "llama3");
        answers.user_name = Some("Sam".to_string());

        write_config(dir.path(), &answers).await.unwrap();

        let user_md =
            std::fs::read_to_string(dir.path().join("assistant").join("USER.md")).unwrap();
        assert!(
            user_md.contains("Sam"),
            "USER.md should be personalized with the user's name: {user_md}"
        );
    }

    fn answers(agent_name: &str) -> WizardAnswers {
        WizardAnswers {
            user_name: Some("Sam".to_string()),
            agent_name: agent_name.to_string(),
            timezone: "UTC".to_string(),
            provider: ProviderKind::Ollama,
            api_key: None,
            model: "llama3".to_string(),
            web_search_backend: None,
            web_search_api_key: None,
            web_search_base_url: None,
        }
    }

    #[tokio::test]
    async fn write_config_creates_a_discoverable_agent() {
        let root = tempfile::tempdir().unwrap();
        write_config(root.path(), &answers("scout")).await.unwrap();

        assert_eq!(
            crate::config::discover_agents(root.path()).unwrap(),
            vec!["scout".to_string()]
        );
        let providers =
            std::fs::read_to_string(root.path().join("scout/config/providers.toml")).unwrap();
        assert!(providers.contains("ollama/llama3"));
    }

    #[tokio::test]
    async fn interrupted_write_config_leaves_no_discoverable_agent() {
        let root = tempfile::tempdir().unwrap();
        // A directory where providers.toml must go makes that write fail
        // before config.toml, the discovery marker, is written.
        std::fs::create_dir_all(root.path().join("scout/config/providers.toml")).unwrap();

        assert!(write_config(root.path(), &answers("scout")).await.is_err());

        assert!(!root.path().join("scout/config/config.toml").exists());
        assert!(
            crate::config::discover_agents(root.path())
                .unwrap()
                .is_empty()
        );
    }
}
