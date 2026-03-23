//! In-place root config reload: diff old vs new config and update changed subsystems.

use std::sync::Arc;

use tokio::time::Duration;

use super::helpers::publish_notice;
use crate::background::spawn_context::SpawnContext;
use crate::config::Config;
use crate::gateway::startup;
use crate::models::CompletionOptions;

use crate::gateway::types::GatewayRuntime;
use crate::gateway::types::GatewayState;
use crate::tunnel::TunnelStatus;

/// What the event loop should do with the idle timer after a config reload.
pub(super) enum IdleAction {
    /// No idle-related changes.
    None,
    /// Idle system was disabled (timeout set to zero).
    Disable,
    /// Idle timeout changed; recalculate the deadline.
    Recalculate { new_timeout: Duration },
}

/// Which subsystems differ between two `Config` snapshots.
#[expect(
    clippy::struct_excessive_bools,
    reason = "diff struct deliberately uses bool flags for each subsystem"
)]
#[derive(PartialEq, Default)]
pub(super) struct ConfigDiff {
    /// Provider chains changed (main, observer, reflector, pulse, embedding, retry, `max_tokens`, temperature, thinking).
    pub providers_changed: bool,
    /// Memory thresholds changed (observer/reflector thresholds, search config).
    pub memory_changed: bool,
    /// Gateway bind/port changed.
    pub gateway_changed: bool,
    /// Discord token added/removed/changed.
    pub discord_changed: bool,
    /// Telegram token added/removed/changed.
    pub telegram_changed: bool,
    /// Pulse enabled/disabled toggle changed.
    pub pulse_changed: bool,
    /// Background task config changed (`max_concurrent`, models).
    pub background_changed: bool,
    /// Agent ability gates changed.
    pub agent_changed: bool,
    /// Skill directories changed.
    pub skills_changed: bool,
    /// Idle system config changed (timeout or `idle_channel`).
    pub idle_changed: bool,
    /// Cloud tunnel config changed (added/removed/changed).
    pub cloud_changed: bool,
}

impl ConfigDiff {
    /// Returns `true` if nothing changed between old and new config.
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Build a human-readable summary of what changed.
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.providers_changed {
            parts.push("providers");
        }
        if self.memory_changed {
            parts.push("memory thresholds");
        }
        if self.gateway_changed {
            parts.push("gateway bind/port");
        }
        if self.discord_changed {
            parts.push("discord");
        }
        if self.telegram_changed {
            parts.push("telegram");
        }
        if self.pulse_changed {
            parts.push("pulse");
        }
        if self.background_changed {
            parts.push("background");
        }
        if self.agent_changed {
            parts.push("agent abilities");
        }
        if self.skills_changed {
            parts.push("skills");
        }
        if self.idle_changed {
            parts.push("idle");
        }
        if self.cloud_changed {
            parts.push("cloud");
        }
        if parts.is_empty() {
            "no changes detected".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// Compare two configs and return which subsystems differ.
pub(super) fn diff_config(old: &Config, new: &Config) -> ConfigDiff {
    ConfigDiff {
        providers_changed: old.main != new.main
            || old.observer != new.observer
            || old.reflector != new.reflector
            || old.pulse != new.pulse
            || old.embedding != new.embedding
            || old.retry != new.retry
            || old.max_tokens != new.max_tokens
            || old.temperature != new.temperature
            || old.thinking != new.thinking
            || old.role_overrides != new.role_overrides,
        memory_changed: old.memory != new.memory,
        gateway_changed: old.gateway != new.gateway,
        discord_changed: old.discord != new.discord,
        telegram_changed: old.telegram != new.telegram,
        pulse_changed: old.pulse_enabled != new.pulse_enabled,
        background_changed: old.background != new.background,
        agent_changed: old.agent != new.agent,
        skills_changed: old.skills != new.skills,
        idle_changed: old.idle != new.idle,
        cloud_changed: old.cloud != new.cloud,
    }
}

/// Backup `config.toml` and `providers.toml` before reload.
///
/// Best-effort: logs a warning on failure but never panics.
pub fn backup_config(config_dir: &std::path::Path) {
    for name in &["config.toml", "providers.toml"] {
        let src = config_dir.join(name);
        let dst = config_dir.join(format!("{name}.bak"));
        if src.exists() {
            if let Err(err) = std::fs::copy(&src, &dst) {
                tracing::warn!(file = %name, error = %err, "failed to back up before reload");
            } else {
                tracing::debug!(file = %name, "backed up to .bak");
            }
        }
    }
}

/// Restore `.bak` files for `config.toml` and `providers.toml` after a failed reload.
///
/// Returns `true` if at least one file was restored successfully.
pub fn rollback_config(config_dir: &std::path::Path) -> bool {
    let mut any_restored = false;
    for name in &["config.toml", "providers.toml"] {
        let backup = config_dir.join(format!("{name}.bak"));
        let target = config_dir.join(name);
        if !backup.exists() {
            continue;
        }
        match std::fs::copy(&backup, &target) {
            Ok(_) => {
                tracing::info!(file = %name, "restored from backup");
                any_restored = true;
            }
            Err(err) => {
                tracing::warn!(file = %name, error = %err, "failed to restore from backup");
            }
        }
    }
    if !any_restored {
        tracing::warn!("no config backups found, cannot rollback");
    }
    any_restored
}

/// Shut down an adapter task and wait up to 5 seconds for it to stop.
async fn shutdown_adapter(
    shutdown_tx: &mut Option<tokio::sync::watch::Sender<bool>>,
    handle: &mut Option<tokio::task::JoinHandle<()>>,
    name: &str,
) {
    if let Some(tx) = shutdown_tx.take() {
        tx.send(true).ok();
    }
    if let Some(h) = handle.take() {
        if tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .is_ok()
        {
            tracing::info!("{name} adapter stopped");
        } else {
            tracing::warn!("{name} adapter shutdown timed out after 5s");
        }
    }
}

/// Handle an in-place root config reload.
///
/// Backs up current config files, loads new config, diffs old vs new, and
/// applies only the changed subsystems. On failure, rolls back and notifies
/// clients.
pub(super) async fn handle_root_reload(rt: &mut GatewayRuntime) -> IdleAction {
    tracing::info!("handling root config reload in-place");
    backup_config(&rt.config_dir);

    let new_cfg = match Config::load_at(&rt.config_dir) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(error = %err, "config reload failed, keeping current config");
            rollback_config(&rt.config_dir);
            publish_notice(
                &rt.publisher,
                format!("config reload failed (keeping current config): {err}"),
            )
            .await;
            return IdleAction::None;
        }
    };

    let diff = diff_config(&rt.cfg, &new_cfg);

    if diff.is_empty() {
        publish_notice(
            &rt.publisher,
            "configuration reloaded: no changes detected".to_string(),
        )
        .await;
        tracing::info!("config reload: no changes detected");
        return IdleAction::None;
    }

    let summary = diff.summary();

    if diff.providers_changed {
        reload_providers(rt, &new_cfg).await;
    }
    if diff.memory_changed {
        reload_memory_thresholds(rt, &new_cfg);
    }
    if diff.gateway_changed {
        reload_gateway(rt, &new_cfg).await;
    }
    if diff.discord_changed {
        reload_discord_adapter(rt, &new_cfg).await;
    }
    if diff.telegram_changed {
        reload_telegram_adapter(rt, &new_cfg).await;
    }
    if diff.cloud_changed {
        reload_tunnel(rt, &new_cfg).await;
    }
    if diff.pulse_changed {
        rt.pulse_enabled = new_cfg.pulse_enabled;
        tracing::info!(enabled = new_cfg.pulse_enabled, "pulse toggle updated");
    }
    if diff.background_changed && !diff.providers_changed {
        reload_background_config(rt, &new_cfg);
    }
    if diff.skills_changed {
        reload_skills(rt).await;
    }
    if diff.agent_changed {
        reload_agent_abilities(rt, &new_cfg).await;
    }

    // ── Store new config ────────────────────────────────────────────────
    rt.cfg = new_cfg;

    publish_notice(&rt.publisher, format!("configuration reloaded: {summary}")).await;
    tracing::info!(changes = %summary, "configuration reloaded successfully");

    if diff.idle_changed {
        if rt.cfg.idle.timeout.is_zero() {
            IdleAction::Disable
        } else {
            IdleAction::Recalculate {
                new_timeout: rt.cfg.idle.timeout,
            }
        }
    } else {
        IdleAction::None
    }
}

/// Build a new `SpawnContext` from the current runtime and new config.
fn build_spawn_context(rt: &GatewayRuntime, new_cfg: &Config) -> Arc<SpawnContext> {
    Arc::new(SpawnContext {
        background_config: new_cfg.background.clone(),
        main_provider_specs: new_cfg.main.clone(),
        http_client: rt.http_client.clone(),
        max_tokens: new_cfg.max_tokens,
        retry_config: new_cfg.retry.clone(),
        identity: rt.spawn_context.identity.clone(),
        options: CompletionOptions {
            max_tokens: Some(new_cfg.max_tokens),
            temperature: new_cfg.temperature,
            thinking: new_cfg.thinking.clone(),
            ..CompletionOptions::default()
        },
        layout: rt.layout.clone(),
        tz: rt.tz,
        role_overrides: new_cfg.role_overrides.clone(),
        background_spawner: Arc::clone(&rt.background_spawner),
        endpoint_registry: rt.endpoint_registry.clone(),
        publisher: rt.publisher.clone(),
        action_store: Arc::clone(&rt.action_store),
        action_notify: Arc::clone(&rt.action_notify),
        hybrid_searcher: Arc::clone(&rt.hybrid_searcher),
    })
}

/// Rebuild providers and swap them into the runtime.
async fn reload_providers(rt: &mut GatewayRuntime, new_cfg: &Config) {
    match startup::init_providers(new_cfg, rt.tz, rt.http_client.clone()) {
        Ok(components) => {
            rt.agent
                .swap_provider(components.provider, components.options);
            rt.observer = components.observer;
            rt.reflector = components.reflector;
            rt.embedding_provider = components.embedding_provider;
            rt.spawn_context = build_spawn_context(rt, new_cfg);
            tracing::info!("providers swapped successfully");
        }
        Err(err) => {
            tracing::warn!(error = %err, "provider rebuild failed, keeping current providers");
            publish_notice(
                &rt.publisher,
                format!("provider rebuild failed (keeping current): {err}"),
            )
            .await;
        }
    }
}

/// Update observer and reflector thresholds from the new config.
fn reload_memory_thresholds(rt: &mut GatewayRuntime, new_cfg: &Config) {
    use crate::memory::observer::ObserverConfig;
    use crate::memory::reflector::ReflectorConfig;

    rt.observer.update_config(ObserverConfig {
        threshold_tokens: new_cfg.memory.observer_threshold_tokens,
        cooldown_secs: new_cfg.memory.observer_cooldown_secs,
        force_threshold_tokens: new_cfg.memory.observer_force_threshold_tokens,
        tz: new_cfg.timezone,
        role_overrides: new_cfg.role_overrides.get("observer").cloned(),
    });

    rt.reflector.update_config(ReflectorConfig {
        threshold_tokens: new_cfg.memory.reflector_threshold_tokens,
        tz: new_cfg.timezone,
        role_overrides: new_cfg.role_overrides.get("reflector").cloned(),
    });

    tracing::info!("memory thresholds updated");
}

/// Rebind the gateway HTTP server to a new address.
async fn reload_gateway(rt: &mut GatewayRuntime, new_cfg: &Config) {
    let new_addr = new_cfg.gateway.addr();
    match tokio::net::TcpListener::bind(&new_addr).await {
        Ok(listener) => {
            rt.http_shutdown_tx.send(true).ok();

            let new_shutdown_tx = tokio::sync::watch::channel::<bool>(false).0;

            let state = GatewayState {
                reload_tx: rt.reload_tx.clone(),
                command_tx: rt.command_tx.clone(),
                inbox_dir: rt.layout.inbox_dir(),
                tz: rt.tz,
                tunnel_status_rx: rt.tunnel_status_rx.clone(),
                publisher: rt.publisher.clone(),
                bus_handle: rt.bus_handle.clone(),
            };
            let config_api_state = crate::gateway::web::ConfigApiState {
                config_dir: rt.config_dir.clone(),
                workspace_dir: rt.layout.root().to_path_buf(),
                memory_dir: Some(rt.layout.memory_dir()),
                reload_tx: Some(rt.reload_tx.clone()),
                setup_done: None,
                secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            };
            let update_api_state = crate::gateway::web::update::UpdateApiState {
                update_status: std::sync::Arc::clone(&rt.update_status),
                restart_tx: rt.restart_tx.clone(),
                gateway_shutdown_tx: rt.gateway_shutdown_tx.clone(),
            };
            let app = crate::gateway::event_loop::build_gateway_app(
                state,
                new_cfg,
                config_api_state,
                update_api_state,
            );

            let new_handle = crate::gateway::event_loop::spawn_server_with_listener(
                listener,
                app,
                &new_shutdown_tx,
            );

            rt.server_handle = new_handle;
            rt.http_shutdown_tx = new_shutdown_tx;
            tracing::info!(addr = %new_addr, "gateway rebound to new address");
        }
        Err(e) => {
            tracing::warn!(
                addr = %new_addr,
                error = %e,
                "failed to bind to new gateway address, keeping current server"
            );
            publish_notice(
                &rt.publisher,
                format!("gateway rebind failed ({new_addr}): {e} — keeping current server"),
            )
            .await;
        }
    }
}

/// Rebuild `SpawnContext` when only background config changed (providers unchanged).
fn reload_background_config(rt: &mut GatewayRuntime, new_cfg: &Config) {
    rt.spawn_context = build_spawn_context(rt, new_cfg);
    tracing::info!("background config updated");
}

/// Rescan skill directories.
async fn reload_skills(rt: &mut GatewayRuntime) {
    let mut skill_guard = rt.skill_state.lock().await;
    if let Err(err) = skill_guard.rescan(None).await {
        tracing::warn!(error = %err, "skill rescan failed during reload");
    } else {
        tracing::info!("skills rescanned");
    }
}

/// Update path policy with new agent ability gates.
async fn reload_agent_abilities(rt: &mut GatewayRuntime, new_cfg: &Config) {
    let mut blocked: Vec<std::path::PathBuf> = vec![
        new_cfg.config_dir.join("config.toml"),
        new_cfg.config_dir.join("config.example.toml"),
        new_cfg.config_dir.join("providers.toml"),
        new_cfg.config_dir.join("providers.example.toml"),
    ];
    if !new_cfg.agent.modify_mcp {
        blocked.push(rt.layout.mcp_json());
    }
    if !new_cfg.agent.modify_channels {
        blocked.push(rt.layout.channels_toml());
    }
    rt.path_policy
        .write()
        .await
        .set_blocked_paths(blocked.into_iter().collect());
    tracing::info!(
        modify_mcp = new_cfg.agent.modify_mcp,
        modify_channels = new_cfg.agent.modify_channels,
        "agent ability gates updated"
    );
}

/// Shut down an adapter and optionally start a replacement using the provided build closure.
///
/// If `build` is `Some`, spawns a new adapter task and records the handle and shutdown sender.
/// If `build` is `None`, the adapter is stopped and not restarted.
async fn reload_adapter<F, Fut>(
    shutdown_tx: &mut Option<tokio::sync::watch::Sender<bool>>,
    handle: &mut Option<tokio::task::JoinHandle<()>>,
    name: &'static str,
    build: Option<F>,
) where
    F: FnOnce(tokio::sync::watch::Receiver<bool>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    shutdown_adapter(shutdown_tx, handle, name).await;
    match build {
        Some(build_fn) => {
            let (tx, rx) = tokio::sync::watch::channel(false);
            *handle = Some(crate::util::spawn_monitored(name, build_fn(rx)));
            *shutdown_tx = Some(tx);
            tracing::info!("{name} adapter restarted with new token");
        }
        None => {
            tracing::info!("{name} adapter removed from config");
        }
    }
}

/// Stop the existing Discord adapter (if running) and start a new one if configured.
async fn reload_discord_adapter(rt: &mut GatewayRuntime, new_cfg: &Config) {
    let senders = crate::gateway::event_loop::AdapterSenders {
        publisher: rt.publisher.clone(),
        bus_handle: rt.bus_handle.clone(),
        reload: rt.reload_tx.clone(),
        command: rt.command_tx.clone(),
    };
    reload_adapter(
        &mut rt.discord_shutdown_tx,
        &mut rt.discord_handle,
        "discord",
        new_cfg.discord.as_ref().map(|cfg| {
            let cfg = cfg.clone();
            let workspace_dir = new_cfg.workspace_dir.clone();
            let tz = rt.tz;
            move |rx: tokio::sync::watch::Receiver<bool>| async move {
                let iface = crate::interfaces::discord::DiscordInterface::new(
                    cfg,
                    senders,
                    workspace_dir,
                    tz,
                    rx,
                );
                if let Err(e) = iface.start().await {
                    tracing::error!(error = %e, "discord interface failed after reload");
                }
            }
        }),
    )
    .await;
}

/// Stop the existing tunnel (if running) and start a new one if configured.
async fn reload_tunnel(rt: &mut GatewayRuntime, new_cfg: &Config) {
    if let Some(tx) = rt.tunnel_shutdown_tx.take() {
        tx.send(true).ok();
    }
    if let Some(handle) = rt.tunnel_handle.take() {
        if tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .is_ok()
        {
            tracing::info!("tunnel stopped");
        } else {
            tracing::warn!("tunnel shutdown timed out after 5s");
        }
    }

    // Ensure status reflects disconnected after old tunnel shutdown
    rt.tunnel_status_tx.send(TunnelStatus::Disconnected).ok();

    if let Some(ref cloud_cfg) = new_cfg.cloud {
        let cloud = cloud_cfg.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let status_tx = std::sync::Arc::clone(&rt.tunnel_status_tx);
        rt.tunnel_handle = Some(crate::util::spawn_monitored("tunnel", async move {
            crate::tunnel::start_tunnel(cloud, shutdown_rx, status_tx).await;
        }));
        rt.tunnel_shutdown_tx = Some(shutdown_tx);
        rt.cloud_config.clone_from(&new_cfg.cloud);
        tracing::info!("tunnel restarted with new config");
    } else {
        rt.cloud_config = None;
        tracing::info!("cloud tunnel removed from config");
    }
}

/// Stop the existing Telegram adapter (if running) and start a new one if configured.
async fn reload_telegram_adapter(rt: &mut GatewayRuntime, new_cfg: &Config) {
    let senders = crate::gateway::event_loop::AdapterSenders {
        publisher: rt.publisher.clone(),
        bus_handle: rt.bus_handle.clone(),
        reload: rt.reload_tx.clone(),
        command: rt.command_tx.clone(),
    };
    reload_adapter(
        &mut rt.telegram_shutdown_tx,
        &mut rt.telegram_handle,
        "telegram",
        new_cfg.telegram.as_ref().map(|cfg| {
            let cfg = cfg.clone();
            let workspace_dir = new_cfg.workspace_dir.clone();
            let tz = rt.tz;
            move |rx: tokio::sync::watch::Receiver<bool>| async move {
                let iface = crate::interfaces::telegram::TelegramInterface::new(
                    cfg,
                    senders,
                    workspace_dir,
                    tz,
                    rx,
                );
                if let Err(e) = iface.start().await {
                    tracing::error!(error = %e, "telegram interface failed after reload");
                }
            }
        }),
    )
    .await;
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::config::{
        AgentAbilitiesConfig, BackgroundConfig, CloudConfig, DiscordConfig, GatewayConfig,
        MemoryConfig, SkillsConfig, TelegramConfig,
    };
    use crate::models::retry::RetryConfig;

    /// Build a minimal test config.
    fn test_config() -> Config {
        Config {
            name: None,
            main: vec![],
            observer: vec![],
            reflector: vec![],
            pulse: vec![],
            embedding: None,
            workspace_dir: std::path::PathBuf::from("/tmp/test"),
            timeout_secs: 30,
            max_tokens: 4096,
            memory: MemoryConfig::default(),
            pulse_enabled: false,
            gateway: GatewayConfig::default(),
            timezone: chrono_tz::UTC,
            cloud: None,
            discord: None,
            telegram: None,
            webhooks: std::collections::HashMap::new(),
            skills: SkillsConfig { dirs: vec![] },
            retry: RetryConfig::default(),
            background: BackgroundConfig::default(),
            agent: AgentAbilitiesConfig::default(),
            idle: crate::config::IdleConfig::default(),
            temperature: None,
            thinking: None,
            web_search: crate::config::WebSearchConfig::default(),
            role_overrides: std::collections::HashMap::new(),
            config_dir: std::path::PathBuf::from("/tmp/config"),
        }
    }

    #[test]
    fn diff_config_no_changes() {
        let cfg = test_config();
        let diff = diff_config(&cfg, &cfg);

        assert!(!diff.providers_changed);
        assert!(!diff.memory_changed);
        assert!(!diff.gateway_changed);
        assert!(!diff.discord_changed);
        assert!(!diff.telegram_changed);
        assert!(!diff.pulse_changed);
        assert!(!diff.background_changed);
        assert!(!diff.agent_changed);
        assert!(!diff.skills_changed);
        assert!(!diff.idle_changed);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_config_detects_provider_change() {
        let old = test_config();
        let mut new = old.clone();
        new.max_tokens = 8192;

        let diff = diff_config(&old, &new);
        assert!(diff.providers_changed);
        assert!(!diff.memory_changed);
    }

    #[test]
    fn diff_config_detects_memory_change() {
        let old = test_config();
        let mut new = old.clone();
        new.memory.observer_threshold_tokens = 999;

        let diff = diff_config(&old, &new);
        assert!(diff.memory_changed);
        assert!(!diff.providers_changed);
    }

    #[test]
    fn diff_config_detects_gateway_change() {
        let old = test_config();
        let mut new = old.clone();
        new.gateway.port = 9999;

        let diff = diff_config(&old, &new);
        assert!(diff.gateway_changed);
        assert!(!diff.providers_changed);
    }

    #[test]
    fn diff_config_multiple_changes() {
        let old = test_config();
        let mut new = old.clone();
        new.max_tokens = 8192;
        new.memory.observer_threshold_tokens = 999;
        new.pulse_enabled = true;
        new.discord = Some(DiscordConfig {
            token: "new-token".to_string(),
        });
        new.telegram = Some(TelegramConfig {
            token: "tg-token".to_string(),
        });
        new.skills.dirs = vec![std::path::PathBuf::from("/new/skills")];
        new.agent.modify_mcp = false;
        new.background.max_concurrent = 10;
        new.cloud = Some(CloudConfig {
            relay_url: "wss://example.com".to_string(),
            token: "tok".to_string(),
            local_port: 7700,
        });
        new.idle.timeout = std::time::Duration::from_secs(300);

        let diff = diff_config(&old, &new);
        assert!(diff.providers_changed);
        assert!(diff.memory_changed);
        assert!(diff.pulse_changed);
        assert!(diff.discord_changed);
        assert!(diff.telegram_changed);
        assert!(diff.skills_changed);
        assert!(diff.agent_changed);
        assert!(diff.background_changed);
        assert!(diff.cloud_changed);
        assert!(diff.idle_changed);
        assert!(!diff.gateway_changed);

        let summary = diff.summary();
        assert!(summary.contains("providers"));
        assert!(summary.contains("memory"));
        assert!(summary.contains("pulse"));
        assert!(summary.contains("discord"));
        assert!(summary.contains("telegram"));
        assert!(summary.contains("skills"));
        assert!(summary.contains("agent"));
        assert!(summary.contains("background"));
        assert!(summary.contains("cloud"));
        assert!(summary.contains("idle"));
    }

    #[test]
    fn diff_config_detects_discord_addition() {
        let old = test_config();
        let mut new = old.clone();
        new.discord = Some(DiscordConfig {
            token: "new-token".to_string(),
        });

        let diff = diff_config(&old, &new);
        assert!(diff.discord_changed);
        assert!(!diff.telegram_changed);
    }

    #[test]
    fn diff_config_detects_discord_removal() {
        let mut old = test_config();
        old.discord = Some(DiscordConfig {
            token: "existing-token".to_string(),
        });
        let mut new = old.clone();
        new.discord = None;

        let diff = diff_config(&old, &new);
        assert!(diff.discord_changed);
    }

    #[test]
    fn diff_config_detects_telegram_token_change() {
        let mut old = test_config();
        old.telegram = Some(TelegramConfig {
            token: "old-tg-token".to_string(),
        });
        let mut new = old.clone();
        new.telegram = Some(TelegramConfig {
            token: "new-tg-token".to_string(),
        });

        let diff = diff_config(&old, &new);
        assert!(diff.telegram_changed);
        assert!(!diff.discord_changed);
    }

    #[test]
    fn diff_config_detects_idle_timeout_change() {
        let old = test_config();
        let mut new = old.clone();
        new.idle.timeout = std::time::Duration::from_secs(600);

        let diff = diff_config(&old, &new);
        assert!(diff.idle_changed);
        assert!(!diff.providers_changed);
    }

    #[test]
    fn diff_config_detects_idle_channel_change() {
        let old = test_config();
        let mut new = old.clone();
        new.idle.idle_channel = Some("telegram".to_string());

        let diff = diff_config(&old, &new);
        assert!(diff.idle_changed);
        assert!(!diff.providers_changed);
    }

    #[test]
    fn diff_config_no_idle_change() {
        let cfg = test_config();
        let diff = diff_config(&cfg, &cfg);
        assert!(!diff.idle_changed);
    }

    #[test]
    fn backup_config_creates_bak_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let providers = dir.path().join("providers.toml");
        std::fs::write(&config, "timezone = \"UTC\"\n").unwrap();
        std::fs::write(&providers, "# providers\n").unwrap();

        backup_config(dir.path());

        let config_bak = dir.path().join("config.toml.bak");
        assert!(config_bak.exists(), "backup should create config.toml.bak");
        assert_eq!(
            std::fs::read_to_string(&config_bak).unwrap(),
            "timezone = \"UTC\"\n",
            "config.toml backup content should match original"
        );

        let providers_bak = dir.path().join("providers.toml.bak");
        assert!(
            providers_bak.exists(),
            "backup should create providers.toml.bak"
        );
        assert_eq!(
            std::fs::read_to_string(&providers_bak).unwrap(),
            "# providers\n",
            "providers.toml backup content should match original"
        );
    }

    #[test]
    fn rollback_config_restores_original() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let providers = dir.path().join("providers.toml");
        let config_bak = dir.path().join("config.toml.bak");
        let providers_bak = dir.path().join("providers.toml.bak");

        std::fs::write(&config_bak, "timezone = \"UTC\"\n").unwrap();
        std::fs::write(&config, "BROKEN").unwrap();
        std::fs::write(&providers_bak, "# providers\n").unwrap();
        std::fs::write(&providers, "BROKEN").unwrap();

        assert!(rollback_config(dir.path()), "rollback should succeed");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "timezone = \"UTC\"\n",
        );
        assert_eq!(
            std::fs::read_to_string(&providers).unwrap(),
            "# providers\n",
        );
    }

    #[test]
    fn rollback_config_fails_without_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "BROKEN").unwrap();
        assert!(!rollback_config(dir.path()));
    }

    #[test]
    fn backup_config_missing_source_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // No config.toml exists — backup should warn but not panic
        backup_config(dir.path());
        assert!(
            !dir.path().join("config.toml.bak").exists(),
            "no backup should be created when source is missing"
        );
    }

    #[test]
    fn diff_config_detects_cloud_change() {
        let mut old = test_config();
        old.cloud = Some(CloudConfig {
            relay_url: "wss://example.com".to_string(),
            token: "old-token".to_string(),
            local_port: 7700,
        });
        let mut new = old.clone();
        new.cloud = Some(CloudConfig {
            relay_url: "wss://example.com".to_string(),
            token: "new-token".to_string(),
            local_port: 7700,
        });

        let diff = diff_config(&old, &new);
        assert!(diff.cloud_changed);
        assert!(!diff.discord_changed);
    }

    #[test]
    fn diff_config_detects_cloud_addition() {
        let old = test_config();
        let mut new = old.clone();
        new.cloud = Some(CloudConfig {
            relay_url: "wss://example.com".to_string(),
            token: "tok".to_string(),
            local_port: 7700,
        });

        let diff = diff_config(&old, &new);
        assert!(diff.cloud_changed);
    }

    #[test]
    fn diff_config_detects_cloud_removal() {
        let mut old = test_config();
        old.cloud = Some(CloudConfig {
            relay_url: "wss://example.com".to_string(),
            token: "tok".to_string(),
            local_port: 7700,
        });
        let mut new = old.clone();
        new.cloud = None;

        let diff = diff_config(&old, &new);
        assert!(diff.cloud_changed);
    }

    #[test]
    fn backup_config_overwrites_stale_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml.bak"), "old content").unwrap();
        std::fs::write(dir.path().join("config.toml"), "new content").unwrap();

        backup_config(dir.path());

        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml.bak")).unwrap(),
            "new content",
            "backup should overwrite previous backup"
        );
    }
}
