use std::collections::HashMap;

use super::super::constants::{
    DEFAULT_DISCORD_CONTEXT_MESSAGES, DEFAULT_IDLE_TIMEOUT_MINUTES, DEFAULT_TEAMS_CONTEXT_MESSAGES,
    DEFAULT_TEAMS_PORT, DEFAULT_TELEGRAM_CONTEXT_MESSAGES, WEB_UI_ENDPOINT,
};
use super::super::deserialize::{
    AgentConfigFile, DiscordConfigFile, TeamsConfigFile, TelegramConfigFile, WebhookEntryFile,
};
use super::super::secrets::SecretStore;
use super::super::types::{
    DiscordConfig, IdleConfig, TeamsConfig, TelegramConfig, WebhookEntry, WebhookFormat,
    WebhookRouting,
};

pub(super) fn resolve_bot_token(raw_token: Option<&str>, secrets: &SecretStore) -> Option<String> {
    raw_token
        .and_then(|t| super::resolve_secret_value(t, secrets))
        .filter(|t| !t.is_empty())
}

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

pub(super) fn resolve_idle_config(
    file: Option<&AgentConfigFile>,
    telegram: Option<&TelegramConfig>,
    discord: Option<&DiscordConfig>,
    teams: Option<&TeamsConfig>,
    notices: &mut Vec<String>,
) -> IdleConfig {
    let section = file.and_then(|f| f.idle.as_ref());
    let timeout_minutes = section
        .and_then(|s| s.timeout_minutes)
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES);

    let resolved = match section.and_then(|s| s.idle_channel.as_deref()) {
        None | Some("websocket" | WEB_UI_ENDPOINT) => Ok(WEB_UI_ENDPOINT),
        Some(channel @ "telegram") if telegram.is_some() => Ok(channel),
        Some(channel @ "discord") if discord.is_some() => Ok(channel),
        Some(channel @ "teams") if teams.is_some() => Ok(channel),
        Some(channel @ ("telegram" | "discord" | "teams")) => Err((
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

#[must_use]
pub(super) fn resolve_configured_chats(
    file: Option<&AgentConfigFile>,
    secrets: &SecretStore,
    notices: &mut Vec<String>,
) -> (
    Option<DiscordConfig>,
    Option<TelegramConfig>,
    Option<TeamsConfig>,
    IdleConfig,
) {
    let discord = resolve_discord_config(file.and_then(|f| f.discord.as_ref()), secrets);
    let telegram = resolve_telegram_config(file.and_then(|f| f.telegram.as_ref()), secrets);
    let teams = resolve_teams_config(file.and_then(|f| f.teams.as_ref()), secrets, notices);
    let idle = resolve_idle_config(
        file,
        telegram.as_ref(),
        discord.as_ref(),
        teams.as_ref(),
        notices,
    );
    (discord, telegram, teams, idle)
}
