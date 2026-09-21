//! In-place root config reload: diff old vs new config and update changed subsystems.

use std::sync::Arc;

use tokio::time::Duration;

use super::helpers::publish_notice;
use crate::background::spawn_context::SpawnContext;
use crate::config::Config;
use crate::gateway::startup;
use crate::inference::CompletionOptions;
use crate::inference::InferenceError;
use crate::inference::SharedHttpClient;

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
///
/// Only operations that are expensive or user-visibly disruptive get a
/// dedicated flag: rebinding the gateway HTTP listener, restarting the
/// Discord/Telegram/Teams adapters, and restarting the cloud tunnel. Idle
/// timeout/channel changes also get a dedicated flag because they control
/// which `IdleAction` variant `handle_root_reload` returns, not a rebuild.
///
/// Every other subsystem (provider chains, memory thresholds, subconscious,
/// background config, skills, tool PATH, agent ability gates, tracing, the
/// pulse toggle, HTTP client timeout, webhooks, the endpoint registry) is
/// cheap to rebuild and
/// `handle_root_reload` rebuilds all of them unconditionally whenever
/// `changed` is true, in one fixed order — see `rebuild_cheap_components`.
#[expect(
    clippy::struct_excessive_bools,
    reason = "diff struct deliberately uses bool flags for each subsystem that needs gating"
)]
pub(super) struct ConfigDiff {
    /// True if anything at all differs between old and new config.
    pub changed: bool,
    /// Gateway bind/port changed — rebinding the HTTP listener is disruptive.
    pub gateway_changed: bool,
    /// Discord token added/removed/changed — restarting the adapter is user-visible.
    pub discord_changed: bool,
    /// Telegram token added/removed/changed — restarting the adapter is user-visible.
    pub telegram_changed: bool,
    /// Teams config (or the gateway bind it listens on) changed — restarts its listener.
    pub teams_changed: bool,
    /// Cloud tunnel config changed — restarting the tunnel is disruptive.
    pub cloud_changed: bool,
    /// Idle timeout or `idle_channel` changed — controls the `IdleAction` returned to the caller.
    pub idle_changed: bool,
    /// Human-readable summary of every subsystem that changed, for the reload log line.
    summary: String,
}

impl ConfigDiff {
    /// Human-readable summary of what changed between old and new config.
    fn summary(&self) -> &str {
        &self.summary
    }
}

/// Compare two configs and return which subsystems differ.
///
/// The granular per-subsystem comparisons below build `summary` for
/// operator-facing logging; only the fields documented on `ConfigDiff`
/// itself are used to gate any actual reload work.
pub(super) fn diff_config(old: &Config, new: &Config) -> ConfigDiff {
    let gateway_changed = old.gateway != new.gateway;
    let discord_changed = old.discord != new.discord;
    let telegram_changed = old.telegram != new.telegram;
    let teams_changed =
        old.teams != new.teams || (new.teams.is_some() && old.gateway.bind != new.gateway.bind);
    let cloud_changed = old.cloud != new.cloud;
    let idle_changed = old.idle != new.idle;

    let mut parts = Vec::new();
    if old.main != new.main
        || old.observer != new.observer
        || old.reflector != new.reflector
        || old.pulse != new.pulse
        || old.embedding != new.embedding
        || old.retry != new.retry
        || old.max_tokens != new.max_tokens
        || old.temperature != new.temperature
        || old.thinking != new.thinking
        || old.role_overrides != new.role_overrides
    {
        parts.push("providers");
    }
    if old.memory != new.memory {
        parts.push("memory thresholds");
    }
    if gateway_changed {
        parts.push("gateway bind/port");
    }
    if discord_changed {
        parts.push("discord");
    }
    if telegram_changed {
        parts.push("telegram");
    }
    if teams_changed {
        parts.push("teams");
    }
    if old.pulse_enabled != new.pulse_enabled {
        parts.push("pulse");
    }
    if old.subconscious != new.subconscious
        || old.subconscious_settings != new.subconscious_settings
    {
        parts.push("subconscious");
    }
    if old.background != new.background {
        parts.push("background");
    }
    if old.agent != new.agent {
        parts.push("agent abilities");
    }
    if old.skills != new.skills {
        parts.push("skills");
    }
    if old.tools != new.tools {
        parts.push("tool path");
    }
    if idle_changed {
        parts.push("idle");
    }
    if cloud_changed {
        parts.push("cloud");
    }
    if old.tracing != new.tracing {
        parts.push("tracing");
    }
    if old.timeout_secs != new.timeout_secs {
        parts.push("http timeout");
    }
    if old.webhooks != new.webhooks {
        parts.push("webhooks");
    }

    let changed = !parts.is_empty();
    let summary = if changed {
        parts.join(", ")
    } else {
        "no changes detected".to_string()
    };

    ConfigDiff {
        changed,
        gateway_changed,
        discord_changed,
        telegram_changed,
        teams_changed,
        cloud_changed,
        idle_changed,
        summary,
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
            tracing::info!(adapter = %name, "adapter stopped");
        } else {
            tracing::warn!(adapter = %name, "adapter shutdown timed out after 5s");
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

    if !diff.changed {
        publish_notice(
            &rt.publisher,
            "configuration reloaded: no changes detected".to_string(),
        )
        .await;
        tracing::info!("config reload: no changes detected");
        return IdleAction::None;
    }

    let summary = diff.summary().to_string();

    // Cheap, stateless subsystems are rebuilt unconditionally, in one fixed
    // order, regardless of which of them actually changed — see
    // `rebuild_cheap_components`. Only the genuinely disruptive operations
    // below stay flag-gated so they don't fire spuriously.
    rebuild_cheap_components(rt, &new_cfg).await;

    if diff.gateway_changed {
        reload_gateway(rt, &new_cfg).await;
    }
    if diff.discord_changed {
        reload_discord_adapter(rt, &new_cfg).await;
    }
    if diff.telegram_changed {
        reload_telegram_adapter(rt, &new_cfg).await;
    }
    if diff.teams_changed {
        reload_teams_adapter(rt, &new_cfg).await;
    }
    if diff.cloud_changed {
        reload_tunnel(rt, &new_cfg).await;
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

/// Build a fresh HTTP client for the given request timeout.
///
/// Pure aside from the `reqwest::Client` construction: no `GatewayRuntime`
/// access, so it can be tested directly and so its only output — the new
/// client — is what callers thread into everything downstream.
fn rebuild_http_client(timeout_secs: u64) -> Result<SharedHttpClient, InferenceError> {
    let client_config = crate::inference::HttpClientConfig::with_timeout(timeout_secs);
    SharedHttpClient::new(&client_config)
}

/// Rebuild every cheap, stateless subsystem from `new_cfg`, in the one fixed
/// order that keeps dependencies correct.
///
/// The HTTP client is rebuilt first and threaded explicitly into everything
/// that needs it (providers, the subconscious classifier, the spawn
/// context) as a function parameter rather than read back off `rt` —
/// `SharedHttpClient::clone` shares the underlying `Arc<reqwest::Client>`,
/// so anything built from a stale client would keep the old `timeout_secs`
/// forever. Passing the freshly built client by value makes that ordering
/// structural: there is no `rt.http_client` to accidentally read from until
/// this function assigns it.
async fn rebuild_cheap_components(rt: &mut GatewayRuntime, new_cfg: &Config) {
    let http_client = match rebuild_http_client(new_cfg.timeout_secs) {
        Ok(client) => {
            tracing::debug!(timeout_secs = new_cfg.timeout_secs, "http client rebuilt");
            client
        }
        Err(err) => {
            tracing::warn!(error = %err, "http client rebuild failed, keeping current client");
            publish_notice(
                &rt.publisher,
                format!("timeout_secs change failed to apply (keeping current timeout): {err}"),
            )
            .await;
            rt.http_client.clone()
        }
    };
    rt.http_client = http_client.clone();

    reload_providers(rt, new_cfg, http_client.clone()).await;
    rt.spawn_context = build_spawn_context(rt, new_cfg, http_client.clone());
    reload_memory_thresholds(rt, new_cfg);
    rt.pulse_enabled = new_cfg.pulse_enabled;
    rt.subconscious = crate::subconscious::Subconscious::build(new_cfg, &rt.layout, http_client);
    tracing::debug!(
        enabled = rt.subconscious.enabled(),
        "subconscious rebuilt from new config"
    );
    reload_skills(rt).await;
    reload_tools_path(rt, new_cfg).await;
    reload_agent_abilities(rt, new_cfg).await;
    reload_tracing(rt, new_cfg).await;
    rt.webhooks.replace_from_config(&new_cfg.webhooks);
    // Adapters added or removed by this reload must show up in list_endpoints,
    // send_message, and idle switching without a restart.
    rt.endpoint_registry.refresh(new_cfg, &rt.channel_configs);
}

/// Build a new `SpawnContext` from the current runtime and new config.
fn build_spawn_context(
    rt: &GatewayRuntime,
    new_cfg: &Config,
    http_client: SharedHttpClient,
) -> Arc<SpawnContext> {
    Arc::new(SpawnContext {
        background_config: new_cfg.background.clone(),
        main_provider_specs: new_cfg.main.clone(),
        http_client,
        max_tokens: new_cfg.max_tokens,
        retry_config: new_cfg.retry.clone(),
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
        skill_state: Arc::clone(&rt.skill_state),
        mcp_registry: Arc::clone(&rt.mcp_registry),
    })
}

/// Rebuild providers and swap them into the runtime.
///
/// `http_client` must be the already-rebuilt client (see
/// `rebuild_cheap_components`), not `rt.http_client` read fresh here, so a
/// timeout change can't be silently dropped by construction order.
async fn reload_providers(
    rt: &mut GatewayRuntime,
    new_cfg: &Config,
    http_client: SharedHttpClient,
) {
    match startup::init_providers(new_cfg, rt.tz, http_client) {
        Ok(components) => {
            rt.agent
                .swap_provider(components.provider, components.options);
            rt.observer = components.observer;
            rt.reflector = components.reflector;
            rt.embedding_provider = components.embedding_provider;
            tracing::debug!("providers swapped successfully");
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

    tracing::debug!("memory thresholds updated");
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
                stop_tx: rt.stop_tx.clone(),
                agent_inbox_dir: rt.layout.agent_inbox_dir(),
                tz: rt.tz,
                tunnel_status_rx: rt.tunnel_status_rx.clone(),
                publisher: rt.publisher.clone(),
                bus_handle: rt.bus_handle.clone(),
                file_registry: rt.file_registry.clone(),
                webhooks: rt.webhooks.clone(),
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
            let tracing_api_state = crate::gateway::web::tracing_api::TracingApiState {
                service: std::sync::Arc::clone(&rt.tracing_service),
                client_context: std::sync::Arc::new(
                    crate::tracing_service::client_context::gather_for_bug_report(new_cfg),
                ),
            };
            let app = crate::gateway::event_loop::build_gateway_app(
                state,
                config_api_state,
                update_api_state,
                tracing_api_state,
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

/// Rescan skill directories.
async fn reload_skills(rt: &mut GatewayRuntime) {
    let mut skill_guard = rt.skill_state.lock().await;
    if let Err(err) = skill_guard.rescan().await {
        tracing::warn!(error = %err, "skill rescan failed during reload");
    } else {
        tracing::debug!("skills rescanned");
    }
}

/// Update the shared tools-`PATH` handle from the new config.
///
/// The `exec` tool and the MCP stdio spawner read this handle at spawn time, so
/// the `exec` tool picks up the change on its next call. Already-running stdio
/// MCP servers keep the `PATH` they were launched with until they next
/// (re)connect — their environment is fixed at spawn.
async fn reload_tools_path(rt: &GatewayRuntime, new_cfg: &Config) {
    *rt.tools_path.write().await = new_cfg.tools.effective_path();
    tracing::debug!("tool PATH updated from new config");
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
    tracing::debug!(
        modify_mcp = new_cfg.agent.modify_mcp,
        modify_channels = new_cfg.agent.modify_channels,
        "agent ability gates updated"
    );
}

/// Update the tracing service and global log filter from the new config.
async fn reload_tracing(rt: &GatewayRuntime, new_cfg: &Config) {
    rt.tracing_service
        .update_config(new_cfg.tracing.clone())
        .await;
    if let Some(handle) = crate::util::tracing_init::global_filter_handle()
        && let Err(e) = handle.set_filter(new_cfg.tracing.log_level)
    {
        tracing::warn!(error = %e, "failed to update log filter on tracing config reload");
    }
    tracing::debug!(level = %new_cfg.tracing.log_level, "tracing config updated");
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
            tracing::info!(adapter = %name, "adapter restarted with new config");
        }
        None => {
            tracing::info!(adapter = %name, "adapter removed from config");
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
        stop: rt.stop_tx.clone(),
        conversations: rt.endpoint_registry.conversations().clone(),
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
    shutdown_adapter(&mut rt.tunnel_shutdown_tx, &mut rt.tunnel_handle, "tunnel").await;

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
        stop: rt.stop_tx.clone(),
        conversations: rt.endpoint_registry.conversations().clone(),
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

/// Stop the existing Teams adapter (if running) and start a new one if configured.
async fn reload_teams_adapter(rt: &mut GatewayRuntime, new_cfg: &Config) {
    let senders = crate::gateway::event_loop::AdapterSenders {
        publisher: rt.publisher.clone(),
        bus_handle: rt.bus_handle.clone(),
        reload: rt.reload_tx.clone(),
        command: rt.command_tx.clone(),
        stop: rt.stop_tx.clone(),
        conversations: rt.endpoint_registry.conversations().clone(),
    };
    reload_adapter(
        &mut rt.teams_shutdown_tx,
        &mut rt.teams_handle,
        "teams",
        new_cfg.teams.as_ref().map(|cfg| {
            let cfg = cfg.clone();
            let bind = new_cfg.gateway.bind.clone();
            let workspace_dir = new_cfg.workspace_dir.clone();
            let tz = rt.tz;
            move |rx: tokio::sync::watch::Receiver<bool>| async move {
                let iface = crate::interfaces::teams::TeamsInterface::new(
                    cfg,
                    senders,
                    bind,
                    workspace_dir,
                    tz,
                    rx,
                );
                if let Err(e) = iface.start().await {
                    tracing::error!(error = %e, "teams interface failed after reload");
                }
            }
        }),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentAbilitiesConfig, BackgroundConfig, CloudConfig, DiscordConfig, GatewayConfig,
        MemoryConfig, SkillsConfig, TelegramConfig, ToolsConfig,
    };
    use crate::inference::retry::RetryConfig;

    /// Build a minimal test config.
    fn test_config() -> Config {
        Config {
            name: None,
            main: vec![],
            observer: vec![],
            reflector: vec![],
            pulse: vec![],
            subconscious: vec![],
            embedding: None,
            workspace_dir: std::path::PathBuf::from("/tmp/test"),
            timeout_secs: 30,
            max_tokens: 4096,
            memory: MemoryConfig::default(),
            pulse_enabled: false,
            subconscious_settings: crate::config::SubconsciousSettings::default(),
            learning: crate::config::LearningConfig::default(),
            gateway: GatewayConfig::default(),
            timezone: chrono_tz::UTC,
            cloud: None,
            discord: None,
            telegram: None,
            teams: None,
            webhooks: std::collections::HashMap::new(),
            skills: SkillsConfig { dirs: vec![] },
            tools: ToolsConfig { dirs: vec![] },
            retry: RetryConfig::default(),
            background: BackgroundConfig::default(),
            agent: AgentAbilitiesConfig::default(),
            idle: crate::config::IdleConfig::default(),
            temperature: None,
            thinking: None,
            web_search: crate::config::WebSearchConfig::default(),
            tracing: crate::config::TracingConfig::default(),
            role_overrides: std::collections::HashMap::new(),
            config_dir: std::path::PathBuf::from("/tmp/config"),
        }
    }

    /// A disruptive-flags-all-false assertion helper: confirms a change didn't
    /// spuriously trip gateway rebind, adapter restart, or tunnel restart.
    fn assert_no_disruptive_flags(diff: &ConfigDiff) {
        assert!(!diff.gateway_changed, "should not flag gateway rebind");
        assert!(!diff.discord_changed, "should not flag discord restart");
        assert!(!diff.telegram_changed, "should not flag telegram restart");
        assert!(!diff.cloud_changed, "should not flag tunnel restart");
    }

    #[test]
    fn diff_config_no_changes() {
        let cfg = test_config();
        let diff = diff_config(&cfg, &cfg);

        assert!(!diff.changed);
        assert_no_disruptive_flags(&diff);
        assert!(!diff.idle_changed);
        assert_eq!(diff.summary(), "no changes detected");
    }

    #[test]
    fn diff_config_detects_provider_change() {
        let old = test_config();
        let mut new = old.clone();
        new.max_tokens = 8192;

        let diff = diff_config(&old, &new);
        assert!(
            diff.changed,
            "cheap provider change should still set changed"
        );
        assert!(diff.summary().contains("providers"));
        assert_no_disruptive_flags(&diff);
    }

    #[test]
    fn diff_config_detects_memory_change() {
        let old = test_config();
        let mut new = old.clone();
        new.memory.observer_threshold_tokens = 999;

        let diff = diff_config(&old, &new);
        assert!(diff.changed);
        assert!(diff.summary().contains("memory"));
        assert!(!diff.summary().contains("providers"));
        assert_no_disruptive_flags(&diff);
    }

    #[test]
    fn diff_config_subconscious_settings_changed() {
        let old = test_config();
        let mut new = test_config();
        new.subconscious_settings.enabled = true;

        let diff = diff_config(&old, &new);
        assert!(diff.changed);
        assert!(
            diff.summary().contains("subconscious"),
            "enabled toggle should be detected"
        );
        assert!(
            !diff.summary().contains("providers"),
            "settings change alone should not flag providers"
        );
    }

    #[test]
    fn diff_config_detects_gateway_change() {
        let old = test_config();
        let mut new = old.clone();
        new.gateway.port = 9999;

        let diff = diff_config(&old, &new);
        assert!(diff.changed);
        assert!(diff.gateway_changed);
        assert!(!diff.discord_changed);
        assert!(!diff.telegram_changed);
        assert!(!diff.cloud_changed);
        assert!(diff.summary().contains("gateway"));
    }

    #[test]
    fn diff_config_detects_discord_addition() {
        let old = test_config();
        let mut new = old.clone();
        new.discord = Some(DiscordConfig {
            token: "new-token".to_string(),
            respond_to_others: false,
        });

        let diff = diff_config(&old, &new);
        assert!(diff.discord_changed);
        assert!(!diff.telegram_changed);
        assert!(!diff.gateway_changed);
        assert!(!diff.cloud_changed);
    }

    #[test]
    fn diff_config_detects_discord_removal() {
        let mut old = test_config();
        old.discord = Some(DiscordConfig {
            token: "existing-token".to_string(),
            respond_to_others: false,
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
            respond_to_others: false,
        });
        let mut new = old.clone();
        new.telegram = Some(TelegramConfig {
            token: "new-tg-token".to_string(),
            respond_to_others: false,
        });

        let diff = diff_config(&old, &new);
        assert!(diff.telegram_changed);
        assert!(!diff.discord_changed);
    }

    fn teams_config() -> crate::config::TeamsConfig {
        crate::config::TeamsConfig {
            app_id: "app".to_string(),
            app_password: "secret".to_string(),
            tenant_id: "tenant".to_string(),
            respond_to_others: false,
            context_messages: 20,
            port: 7701,
        }
    }

    #[test]
    fn diff_config_restarts_teams_on_its_own_changes() {
        let mut old = test_config();
        old.teams = Some(teams_config());
        let mut new = old.clone();
        new.teams = Some(crate::config::TeamsConfig {
            respond_to_others: true,
            ..teams_config()
        });

        let diff = diff_config(&old, &new);
        assert!(diff.teams_changed);
        assert!(diff.summary().contains("teams"));
        assert!(!diff.telegram_changed);
    }

    #[test]
    fn diff_config_restarts_teams_when_the_bind_it_shares_changes() {
        let mut old = test_config();
        old.teams = Some(teams_config());
        let mut new = old.clone();
        new.gateway.bind = "0.0.0.0".to_string();
        assert!(diff_config(&old, &new).teams_changed);

        // Without Teams configured, a bind change is only a gateway change.
        old.teams = None;
        new.teams = None;
        assert!(!diff_config(&old, &new).teams_changed);
    }

    #[test]
    fn diff_config_detects_idle_timeout_change() {
        let old = test_config();
        let mut new = old.clone();
        new.idle.timeout = std::time::Duration::from_mins(10);

        let diff = diff_config(&old, &new);
        assert!(diff.idle_changed);
        assert_no_disruptive_flags(&diff);
    }

    #[test]
    fn diff_config_detects_idle_channel_change() {
        let old = test_config();
        let mut new = old.clone();
        new.idle.idle_channel = Some("telegram".to_string());

        let diff = diff_config(&old, &new);
        assert!(diff.idle_changed);
        assert_no_disruptive_flags(&diff);
    }

    #[test]
    fn diff_config_no_idle_change() {
        let cfg = test_config();
        let diff = diff_config(&cfg, &cfg);
        assert!(!diff.idle_changed);
    }

    #[test]
    fn diff_config_detects_http_timeout_change() {
        let old = test_config();
        let mut new = old.clone();
        new.timeout_secs = 90;

        let diff = diff_config(&old, &new);
        assert!(diff.changed, "timeout_secs change should be detected");
        assert!(diff.summary().contains("http timeout"));
        assert!(
            !diff.summary().contains("providers"),
            "timeout_secs alone should not flag providers"
        );
        assert_no_disruptive_flags(&diff);
    }

    #[test]
    fn diff_config_multiple_cheap_changes() {
        let old = test_config();
        let mut new = old.clone();
        new.max_tokens = 8192;
        new.memory.observer_threshold_tokens = 999;
        new.pulse_enabled = true;
        new.skills.dirs = vec![std::path::PathBuf::from("/new/skills")];
        new.agent.modify_mcp = false;
        new.background.max_concurrent = 10;
        new.idle.timeout = std::time::Duration::from_mins(5);

        let diff = diff_config(&old, &new);
        assert!(diff.changed);
        assert!(diff.idle_changed);
        assert_no_disruptive_flags(&diff);

        let summary = diff.summary();
        assert!(summary.contains("providers"));
        assert!(summary.contains("memory"));
        assert!(summary.contains("pulse"));
        assert!(summary.contains("skills"));
        assert!(summary.contains("agent"));
        assert!(summary.contains("background"));
        assert!(summary.contains("idle"));
        assert!(!summary.contains("discord"));
        assert!(!summary.contains("telegram"));
        assert!(!summary.contains("cloud"));
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
    fn rebuild_http_client_uses_new_timeout() {
        let client = rebuild_http_client(90).expect("client build should succeed");
        assert_eq!(
            client.timeout_secs(),
            90,
            "rebuilt client should carry the requested timeout, not a stale default"
        );
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
