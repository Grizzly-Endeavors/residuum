//! Discord, Telegram, Teams, Slack, webhooks, and idle settings.

use std::collections::HashMap;

use super::super::constants::{
    DEFAULT_DISCORD_CONTEXT_MESSAGES, DEFAULT_IDLE_TIMEOUT_MINUTES, DEFAULT_TEAMS_CONTEXT_MESSAGES,
    DEFAULT_TEAMS_PORT, DEFAULT_TELEGRAM_CONTEXT_MESSAGES, WEB_UI_ENDPOINT,
};
use super::super::deserialize::{
    AgentConfigFile, DiscordConfigFile, SlackConfigFile, TeamsConfigFile, TelegramConfigFile,
    WebhookEntryFile,
};
use super::super::secrets::SecretStore;
use super::super::types::{
    DiscordConfig, IdleConfig, SlackConfig, TeamsConfig, TelegramConfig, WebhookEntry,
    WebhookFormat, WebhookRouting,
};

/// Resolve a bot token from the raw TOML value, with `${ENV_VAR}` / `secret:name`
/// expansion. There are no agent-scoped env var overrides (such as
/// `RESIDUUM_DISCORD_TOKEN`): in a multi-agent hub they would apply the same
/// value to every agent. A set one gets a startup notice — see
/// `removed_agent_env_override_notices`.
pub(super) fn resolve_bot_token(raw_token: Option<&str>, secrets: &SecretStore) -> Option<String> {
    raw_token
        .and_then(|t| super::resolve_secret_value(t, secrets))
        .filter(|t| !t.is_empty())
}

/// Resolve Discord configuration from the TOML section.
///
/// Token resolution: `token` field in TOML (with `${ENV_VAR}` / `secret:name`
/// expansion) > `None` if section is absent or no token found.
pub(super) fn resolve_discord_config(
    section: Option<&DiscordConfigFile>,
    secrets: &SecretStore,
) -> Option<DiscordConfig> {
    let token = resolve_bot_token(section.and_then(|s| s.token.as_deref()), secrets);

    match (section, token) {
        (_, Some(tok)) => Some(DiscordConfig {
            token: tok,
            respond_to_others: section.and_then(|s| s.respond_to_others).unwrap_or(false),
            context_messages: section
                .and_then(|s| s.context_messages)
                .unwrap_or(DEFAULT_DISCORD_CONTEXT_MESSAGES),
            deliver_intermediate: section
                .and_then(|s| s.deliver_intermediate)
                .unwrap_or(false),
        }),
        (Some(_), None) => {
            tracing::warn!(
                section = "discord",
                "section present but no token found; set token in config"
            );
            None
        }
        (None, None) => None,
    }
}

/// Resolve Slack configuration from the TOML section.
///
/// Token resolution: `bot_token` / `app_token` fields in TOML (with
/// `${ENV_VAR}` / `secret:name` expansion). Both are required — the section
/// resolves to `None` unless both are present.
pub(super) fn resolve_slack_config(
    section: Option<&SlackConfigFile>,
    secrets: &SecretStore,
) -> Option<SlackConfig> {
    let bot_token = resolve_bot_token(section.and_then(|s| s.bot_token.as_deref()), secrets);
    let app_token = resolve_bot_token(section.and_then(|s| s.app_token.as_deref()), secrets);

    match (section, bot_token, app_token) {
        (_, Some(bot), Some(app)) => Some(SlackConfig {
            bot_token: bot,
            app_token: app,
            respond_to_others: section.and_then(|s| s.respond_to_others).unwrap_or(false),
            working_reaction: resolve_working_reaction(
                section.and_then(|s| s.working_reaction.as_deref()),
            ),
            deliver_intermediate: section
                .and_then(|s| s.deliver_intermediate)
                .unwrap_or(false),
        }),
        (Some(_), _, _) => {
            tracing::warn!(
                section = "slack",
                "section present but bot_token/app_token incomplete; set both in config"
            );
            None
        }
        (None, _, _) => None,
    }
}

/// The working reaction Slack uses when `[slack] working_reaction` is unset:
/// 👀, the acknowledgement Slack's own agent samples use.
const DEFAULT_SLACK_WORKING_REACTION: &str = "eyes";

/// Resolve `[slack] working_reaction`: unset is the default, `""` turns the
/// reaction off, and surrounding colons (`":zap:"`, as Slack displays a
/// shortcode) are tolerated — the API takes the bare name.
fn resolve_working_reaction(raw: Option<&str>) -> Option<String> {
    let Some(raw) = raw else {
        return Some(DEFAULT_SLACK_WORKING_REACTION.to_string());
    };
    let name = raw.trim().trim_matches(':');
    (!name.is_empty()).then(|| name.to_string())
}

/// Resolve Telegram configuration from the TOML section.
///
/// Token resolution: `token` field in TOML (with `${ENV_VAR}` / `secret:name`
/// expansion) > `None` if section is absent or no token found.
pub(super) fn resolve_telegram_config(
    section: Option<&TelegramConfigFile>,
    secrets: &SecretStore,
) -> Option<TelegramConfig> {
    let token = resolve_bot_token(section.and_then(|s| s.token.as_deref()), secrets);

    match (section, token) {
        (_, Some(tok)) => Some(TelegramConfig {
            token: tok,
            respond_to_others: section.and_then(|s| s.respond_to_others).unwrap_or(false),
            context_messages: section
                .and_then(|s| s.context_messages)
                .unwrap_or(DEFAULT_TELEGRAM_CONTEXT_MESSAGES),
            deliver_intermediate: section
                .and_then(|s| s.deliver_intermediate)
                .unwrap_or(false),
        }),
        (Some(_), None) => {
            tracing::warn!(
                section = "telegram",
                "section present but no token found; set token in config"
            );
            None
        }
        (None, None) => None,
    }
}

/// Resolve Microsoft Teams configuration from the TOML section.
///
/// The client secret comes from the `app_password` field (with `${ENV_VAR}`
/// / `secret:name` expansion).
///
/// Teams is an optional, independent feature: a half-configured `[teams]`
/// section (missing `app_id`, `tenant_id`, or the app password) disables
/// Teams with a notice explaining why, rather than failing the whole
/// config — the rest of the gateway has nothing to do with Teams.
pub(super) fn resolve_teams_config(
    section: Option<&TeamsConfigFile>,
    secrets: &SecretStore,
    notices: &mut Vec<String>,
) -> Option<TeamsConfig> {
    let section = section?;
    let required = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let mut disable = |field: &str| {
        tracing::warn!(field, "[teams] is present but incomplete; disabling teams");
        notices.push(format!(
            "Teams is disabled: [teams] is present but {field} is missing. Set it, or remove the [teams] section, and reload to re-enable Teams."
        ));
    };

    let Some(app_id) = required(section.app_id.as_deref()) else {
        disable("app_id");
        return None;
    };
    let Some(tenant_id) = required(section.tenant_id.as_deref()) else {
        disable("tenant_id");
        return None;
    };
    let Some(app_password) = resolve_bot_token(section.app_password.as_deref(), secrets) else {
        disable("app_password");
        return None;
    };

    Some(TeamsConfig {
        app_id,
        app_password,
        tenant_id,
        respond_to_others: section.respond_to_others.unwrap_or(false),
        context_messages: section
            .context_messages
            .unwrap_or(DEFAULT_TEAMS_CONTEXT_MESSAGES),
        deliver_intermediate: section.deliver_intermediate.unwrap_or(false),
        port: section.port.unwrap_or(DEFAULT_TEAMS_PORT),
    })
}

/// Resolve named webhook configurations from TOML `[webhooks.<name>]` sections.
///
/// Each webhook entry is independent, so a bad `routing`/`format` string or
/// an empty `content_fields` entry drops just that one entry (with a
/// notice) rather than failing every configured webhook.
pub(super) fn resolve_webhooks_config(
    section: Option<&HashMap<String, WebhookEntryFile>>,
    secrets: &SecretStore,
    notices: &mut Vec<String>,
) -> HashMap<String, WebhookEntry> {
    let Some(entries) = section else {
        return HashMap::new();
    };

    let mut result = HashMap::with_capacity(entries.len());

    for (name, entry) in entries {
        if let Some(webhook) = resolve_webhook_entry(name, entry, secrets, notices) {
            result.insert(name.clone(), webhook);
        }
    }

    result
}

/// Resolve one `[webhooks.<name>]` entry, or `None` (with a notice) if its
/// `routing`, `format`, or `content_fields` is invalid.
fn resolve_webhook_entry(
    name: &str,
    entry: &WebhookEntryFile,
    secrets: &SecretStore,
    notices: &mut Vec<String>,
) -> Option<WebhookEntry> {
    let mut skip = |problem: &str| {
        tracing::warn!(webhook = name, problem, "skipping invalid webhook entry");
        notices.push(format!(
            "Skipped webhook \"{name}\": {problem}. Fix it in [webhooks.{name}] and reload to re-enable it."
        ));
    };

    let secret = entry
        .secret
        .as_deref()
        .and_then(|raw| super::resolve_secret_value(raw, secrets));

    let routing: WebhookRouting = match entry.routing.as_deref() {
        Some(s) => match s.parse() {
            Ok(routing) => routing,
            Err(e) => {
                skip(&e);
                return None;
            }
        },
        None => WebhookRouting::default(),
    };

    let format: WebhookFormat = match entry.format.as_deref() {
        Some(s) => match s.parse() {
            Ok(format) => format,
            Err(e) => {
                skip(&e);
                return None;
            }
        },
        None => WebhookFormat::default(),
    };

    if let Some(ref fields) = entry.content_fields {
        for (i, field) in fields.iter().enumerate() {
            if field.trim().is_empty() {
                skip(&format!("content_fields[{i}] is empty"));
                return None;
            }
        }
    }

    Some(WebhookEntry {
        secret,
        routing,
        format,
        content_fields: entry.content_fields.clone(),
    })
}

/// Resolve idle configuration from TOML section, validating the idle channel
/// against configured interfaces.
///
/// An unset `idle_channel` resolves to the web UI (`ws`), so the runtime
/// always has a concrete endpoint for agent-started turns once the owner is
/// idle. One naming an unknown or unconfigured interface also resolves to the
/// web UI, with a notice, rather than failing the whole config over one bad
/// setting.
pub(super) fn resolve_idle_config(
    file: Option<&AgentConfigFile>,
    telegram: Option<&TelegramConfig>,
    discord: Option<&DiscordConfig>,
    teams: Option<&TeamsConfig>,
    slack: Option<&SlackConfig>,
    notices: &mut Vec<String>,
) -> IdleConfig {
    let section = file.and_then(|f| f.idle.as_ref());
    let timeout_minutes = section
        .and_then(|s| s.timeout_minutes)
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES);

    let resolved = match section.and_then(|s| s.idle_channel.as_deref()) {
        // The web UI's endpoint ID is `ws`; `websocket` is the name users see and write.
        None | Some("websocket" | WEB_UI_ENDPOINT) => Ok(WEB_UI_ENDPOINT),
        Some(channel @ "telegram") if telegram.is_some() => Ok(channel),
        Some(channel @ "discord") if discord.is_some() => Ok(channel),
        Some(channel @ "teams") if teams.is_some() => Ok(channel),
        Some(channel @ "slack") if slack.is_some() => Ok(channel),
        Some(channel @ ("telegram" | "discord" | "teams" | "slack")) => Err((
            channel,
            format!("idle_channel \"{channel}\" configured but [{channel}] section is missing"),
        )),
        Some(other) => Err((
            other,
            format!("idle_channel \"{other}\" is not a recognized interface"),
        )),
    };

    let idle_channel = match resolved {
        Ok(channel) => channel.to_string(),
        Err((channel, problem)) => {
            tracing::warn!(
                channel,
                "invalid idle_channel, idle updates go to the web app"
            );
            notices.push(format!(
                "{problem}. While you're away, updates go to the web app until you fix idle_channel and reload."
            ));
            WEB_UI_ENDPOINT.to_string()
        }
    };

    IdleConfig {
        timeout: std::time::Duration::from_secs(timeout_minutes * 60),
        idle_channel,
    }
}

/// The chat interfaces an agent has configured, and the idle channel that
/// names one of them.
pub(super) struct ConfiguredChats {
    pub(super) discord: Option<DiscordConfig>,
    pub(super) telegram: Option<TelegramConfig>,
    pub(super) teams: Option<TeamsConfig>,
    pub(super) slack: Option<SlackConfig>,
    pub(super) idle: IdleConfig,
}

/// Resolve every chat interface and the idle channel.
///
/// Idle validation needs the chat configs, so they are resolved together.
#[must_use]
pub(super) fn resolve_configured_chats(
    file: Option<&AgentConfigFile>,
    secrets: &SecretStore,
    notices: &mut Vec<String>,
) -> ConfiguredChats {
    let discord = resolve_discord_config(file.and_then(|f| f.discord.as_ref()), secrets);
    let telegram = resolve_telegram_config(file.and_then(|f| f.telegram.as_ref()), secrets);
    let teams = resolve_teams_config(file.and_then(|f| f.teams.as_ref()), secrets, notices);
    let slack = resolve_slack_config(file.and_then(|f| f.slack.as_ref()), secrets);
    let idle = resolve_idle_config(
        file,
        telegram.as_ref(),
        discord.as_ref(),
        teams.as_ref(),
        slack.as_ref(),
        notices,
    );
    ConfiguredChats {
        discord,
        telegram,
        teams,
        slack,
        idle,
    }
}
