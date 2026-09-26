//! In-place root config reload: diff old vs new config and update changed subsystems.

use std::sync::Arc;

use tokio::time::Duration;

use super::helpers::publish_notice;
use crate::background::spawn_context::SpawnContext;
use crate::config::Config;
use crate::gateway::{last_known_good, startup};
use crate::inference::CompletionOptions;
use crate::inference::InferenceError;
use crate::inference::SharedHttpClient;

use crate::config::ProviderSpec;
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
/// pulse toggle, HTTP client timeout, webhooks, web search, the endpoint
/// registry) is cheap to rebuild and
/// `handle_root_reload` rebuilds all of them unconditionally whenever
/// `changed` is true, in one fixed order — see `rebuild_cheap_components`.
/// Web search is fully live: `provider_native` is folded into the
/// unconditional provider rebuild, `standalone_backend` naming `"ollama"`
/// reloads the native tool in place, and a `"brave"`/`"tavily"` backend
/// reconnects its MCP server — see `reload_web_search`.
///
/// A credential-only change (a provider's resolved `api_key` differs while
/// its name, model, and URL stay the same) already flips `changed` through
/// the same per-role equality checks as any other provider edit, but the
/// generic "providers"/"background"/"subconscious" label doesn't say *which*
/// provider rotated. `summary` calls that out by name (never by value) —
/// see `provider_credential_changes` — so the reload log/notice reflects
/// what actually happened instead of reading like a no-op.
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
    /// A2A config (or the gateway bind it listens on) changed — restarts its listener.
    pub a2a_changed: bool,
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
    let a2a_changed =
        old.a2a != new.a2a || (new.a2a.enabled && old.gateway.bind != new.gateway.bind);
    let cloud_changed = old.cloud != new.cloud;
    let idle_changed = old.idle != new.idle;

    let parts = summary_parts(old, new);
    let changed = !parts.is_empty();
    let mut summary = if changed {
        parts.join(", ")
    } else {
        "no changes detected".to_string()
    };

    let credential_changes = provider_credential_changes(old, new);
    if !credential_changes.is_empty() {
        summary = format!(
            "{summary}; credential changed for {}",
            credential_changes.join(", ")
        );
    }

    ConfigDiff {
        changed,
        gateway_changed,
        discord_changed,
        telegram_changed,
        teams_changed,
        a2a_changed,
        cloud_changed,
        idle_changed,
        summary,
    }
}

/// Human-readable labels for every subsystem that differs between `old` and
/// `new`, in the order shown in the reload log line. Split out of
/// `diff_config` purely to keep that function's line count bounded; each
/// check here is independent and re-derives its own condition rather than
/// taking `ConfigDiff`'s flags as parameters.
fn summary_parts(old: &Config, new: &Config) -> Vec<&'static str> {
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
    if old.gateway != new.gateway {
        parts.push("gateway bind/port");
    }
    if old.discord != new.discord {
        parts.push("discord");
    }
    if old.telegram != new.telegram {
        parts.push("telegram");
    }
    if old.teams != new.teams || (new.teams.is_some() && old.gateway.bind != new.gateway.bind) {
        parts.push("teams");
    }
    if old.a2a != new.a2a || (new.a2a.enabled && old.gateway.bind != new.gateway.bind) {
        parts.push("a2a");
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
    if old.idle != new.idle {
        parts.push("idle");
    }
    if old.cloud != new.cloud {
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
    if old.web_search != new.web_search {
        parts.push("web search");
    }
    parts
}

/// Provider names (each tagged with its role) whose resolved credential
/// differs between `old` and `new`, across every role that carries a
/// provider chain.
///
/// Reports only that a named provider's credential changed, never the
/// credential values themselves — those stay behind `ProviderSpec`'s
/// redacting `Debug` impl and are never compared by content here beyond a
/// `!=` check.
fn provider_credential_changes(old: &Config, new: &Config) -> Vec<String> {
    let mut changes = Vec::new();
    changes.extend(credential_change_labels("main", &old.main, &new.main));
    changes.extend(credential_change_labels(
        "observer",
        &old.observer,
        &new.observer,
    ));
    changes.extend(credential_change_labels(
        "reflector",
        &old.reflector,
        &new.reflector,
    ));
    changes.extend(credential_change_labels("pulse", &old.pulse, &new.pulse));
    changes.extend(credential_change_labels(
        "subconscious",
        &old.subconscious,
        &new.subconscious,
    ));

    if let (Some(o), Some(n)) = (&old.embedding, &new.embedding)
        && o.name == n.name
        && o.api_key != n.api_key
    {
        changes.push(format!("{} (embedding)", o.name));
    }

    changes.extend(credential_change_labels(
        "background:small",
        old.background.models.small.as_deref().unwrap_or_default(),
        new.background.models.small.as_deref().unwrap_or_default(),
    ));
    changes.extend(credential_change_labels(
        "background:medium",
        old.background.models.medium.as_deref().unwrap_or_default(),
        new.background.models.medium.as_deref().unwrap_or_default(),
    ));
    changes.extend(credential_change_labels(
        "background:large",
        old.background.models.large.as_deref().unwrap_or_default(),
        new.background.models.large.as_deref().unwrap_or_default(),
    ));

    changes
}

/// Provider names whose credential (only) differs between two same-length,
/// same-order provider chains for one role.
///
/// Skips chains that differ in length or provider identity at a position —
/// that's a structural change the per-role equality check already folds
/// into the generic summary label, and pairing by position across a
/// resized or reordered chain would misattribute the change to the wrong
/// provider.
fn credential_change_labels(role: &str, old: &[ProviderSpec], new: &[ProviderSpec]) -> Vec<String> {
    if old.len() != new.len() {
        return Vec::new();
    }
    old.iter()
        .zip(new.iter())
        .filter(|(o, n)| o.name == n.name && o.api_key != n.api_key)
        .map(|(o, _)| format!("{} ({role})", o.name))
        .collect()
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
/// Loads the new config, diffs old vs new, and applies only the changed
/// subsystems. On failure the running config stays in effect, the files on
/// disk are left as the user wrote them, and clients are notified.
pub(super) async fn handle_root_reload(rt: &mut GatewayRuntime) -> IdleAction {
    tracing::info!("handling root config reload in-place");

    // Consumed once, up front: whether this specific reload is the one the
    // agent's own `write_file`/`edit_file` call to config.toml/providers.toml
    // caused — see `ConfigWriteWatch`. Every exit path below delivers the
    // same text it already publishes as a user notice into the agent's own
    // transcript too, when this is `true`.
    let deliver_to_agent = rt
        .config_reload_tracker
        .take_if_matches(crate::tools::config_reload_tracker::ConfigReloadKind::Root);

    let new_cfg = match Config::load_at(&rt.config_dir) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(error = %err, "config reload failed, keeping current config");
            let message = format!("config reload failed (keeping current config): {err}");
            publish_notice(&rt.publisher, message.clone()).await;
            if deliver_to_agent {
                rt.agent.inject_system_message(message);
            }
            return IdleAction::None;
        }
    };
    // Each is already a complete, standalone sentence describing one
    // config.toml/providers.toml entry the new load skipped or degraded
    // (see `config::resolve` and `config::tolerant`).
    for notice in &new_cfg.load_notices {
        publish_notice(&rt.publisher, notice.clone()).await;
    }

    let diff = diff_config(&rt.cfg, &new_cfg);

    if !diff.changed {
        // The live files still load and resolve fine even though nothing
        // changed — worth saving as last-known-good too, in case the
        // previous save predates a since-reverted edit.
        last_known_good::save(&rt.config_dir);
        let message = "configuration reloaded: no changes detected".to_string();
        publish_notice(&rt.publisher, message.clone()).await;
        if deliver_to_agent {
            rt.agent.inject_system_message(message);
        }
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
    if diff.a2a_changed {
        reload_a2a_adapter(rt, &new_cfg).await;
    }
    // An `[a2a]` change also respawns the tunnel: its capabilities (whether
    // `a2a`/`a2a-private` are advertised) are only sent on the tunnel's
    // upgrade, so the relay never sees a visibility flip or an enable/disable
    // without a fresh connection.
    if diff.cloud_changed || diff.a2a_changed {
        reload_tunnel(rt, &new_cfg).await;
    }

    // ── Store new config ────────────────────────────────────────────────
    rt.cfg = new_cfg;

    // Every subsystem above degrades independently and never fails this
    // function outright, so reaching here means the reload fully applied —
    // exactly what "last-known-good" means.
    last_known_good::save(&rt.config_dir);

    let message = format!("configuration reloaded: {summary}");
    publish_notice(&rt.publisher, message.clone()).await;
    if deliver_to_agent {
        rt.agent.inject_system_message(message);
    }
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
    // Pushes to the model-call HTTP endpoint's watch receiver, so `POST
    // /api/model/complete` resolves providers from this reload without the
    // HTTP router being rebuilt. `.ok()`: the only way this fails is no
    // receiver remaining, which can't happen while the server is running.
    rt.model_call_resources_tx
        .send(Arc::new(
            crate::gateway::web::model::ModelCallResources::from_spawn_context(&rt.spawn_context),
        ))
        .ok();
    reload_web_search(rt, new_cfg).await;
    reload_memory_thresholds(rt, new_cfg).await;
    rt.pulse_enabled = new_cfg.pulse_enabled;
    rt.subconscious = crate::subconscious::Subconscious::build(
        new_cfg,
        &rt.layout,
        http_client,
        rt.publisher.clone(),
    );
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
    // Rebuilt fresh so sessions forked after this reload pick up the new
    // `[observer]` config, mirroring `reload_providers`'s treatment of the
    // main agent's own observer. A failed rebuild keeps the previous
    // instance (stale config) rather than losing session extraction
    // entirely — the same degrade-gracefully choice `reload_providers` makes.
    let observer = match startup::init_session_observer(new_cfg, rt.tz, http_client.clone()) {
        Ok(observer) => Arc::new(observer),
        Err(err) => {
            tracing::warn!(error = %err, "session observer rebuild failed, keeping current instance");
            Arc::clone(&rt.spawn_context.observer)
        }
    };

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
        max_tool_iterations: new_cfg.agent.max_tool_iterations,
        repeat_call_guard: new_cfg.agent.repeat_call_guard,
        layout: rt.layout.clone(),
        config_dir: new_cfg.config_dir.clone(),
        tz: rt.tz,
        role_overrides: new_cfg.role_overrides.clone(),
        session_runtime: Arc::clone(&rt.session_runtime),
        session_registry: Arc::clone(&rt.session_registry),
        endpoint_registry: rt.endpoint_registry.clone(),
        publisher: rt.publisher.clone(),
        action_store: Arc::clone(&rt.action_store),
        action_notify: Arc::clone(&rt.action_notify),
        hybrid_searcher: Arc::clone(&rt.hybrid_searcher),
        skill_state: Arc::clone(&rt.skill_state),
        mcp_registry: Arc::clone(&rt.mcp_registry),
        observer,
        merge_writer: Arc::clone(&rt.merge_writer),
        messenger: Arc::clone(&rt.agent_messenger),
        // Shared `Arc`, same instance `reload_tracing` updates in place —
        // sessions forked after this reload see the same config, no rebuild
        // needed here.
        tracing_service: Arc::clone(&rt.tracing_service),
        // Rebuilt fresh from `new_cfg`, mirroring the snapshot `reload_gateway`
        // builds for the HTTP tracing API, so a session forked after this
        // reload reports the currently active model/provider.
        tracing_client_context: Arc::new(
            crate::tracing_service::client_context::gather_for_bug_report(new_cfg),
        ),
        web_search_backend: new_cfg.web_search.standalone_backend.clone(),
        tools_path: Arc::clone(&rt.tools_path),
        path_policy: Arc::clone(&rt.path_policy),
        agent_keys: Arc::clone(&rt.agent_keys),
        a2a_hub: Arc::clone(&rt.a2a_hub),
        a2a_tracker: Arc::clone(&rt.a2a_tracker),
        checkpoints: Arc::clone(&rt.checkpoints),
        bg_tier_active_index: crate::background::spawn_context::BackgroundTierActiveIndex::default(
        ),
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
    let mut degradations: Vec<String> = Vec::new();
    match startup::init_providers(
        new_cfg,
        rt.tz,
        http_client,
        &rt.publisher,
        &mut degradations,
    ) {
        Ok(components) => {
            rt.agent
                .swap_provider(components.provider, components.options);
            rt.observer = Arc::new(components.observer);
            rt.merge_writer.swap_reflector(components.reflector).await;
            rt.merge_writer
                .set_embedding_provider(components.embedding_provider)
                .await;
            tracing::debug!("providers swapped successfully");
            if !degradations.is_empty() {
                publish_notice(
                    &rt.publisher,
                    format!(
                        "providers reloaded, but {} degraded: {}.",
                        degradations.len(),
                        degradations.join("; ")
                    ),
                )
                .await;
            }
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

/// Reload main's standalone web search setup from the new config.
///
/// `ollama_web_search` is a native tool, so it reloads in place via
/// `Agent::reload_ollama_web_search_tool` — called unconditionally, like
/// every other cheap component, since removing and re-adding a
/// not-actually-changed tool is harmless.
///
/// A `"brave"`/`"tavily"` backend is an MCP server instead: when the
/// standalone backend actually changed, this disconnects whichever brave/
/// tavily server was previously running (by name — `connect_servers` skips
/// a name it already tracks, even if the entry behind it, e.g. the API key,
/// changed, so a stale connection would otherwise survive silently) and
/// reconnects via `connect_web_search_mcp`, the same helper startup uses.
/// The MCP registry is shared with every session (see
/// `docs/systems-usage/background-tasks.md`), so this one reconnect updates
/// the tools available to main and every live or future session — no
/// separate per-session step needed. A connect failure surfaces the same
/// way `reload_providers`'s does: a `warn` log plus an operator-facing
/// notice, rather than silently leaving the old (or no) server running.
async fn reload_web_search(rt: &mut GatewayRuntime, new_cfg: &Config) {
    rt.agent
        .reload_ollama_web_search_tool(new_cfg.web_search.standalone_backend.as_ref());

    if rt.cfg.web_search.standalone_backend == new_cfg.web_search.standalone_backend {
        return;
    }

    if let Some(name) = web_search_mcp_server_name(rt.cfg.web_search.standalone_backend.as_ref()) {
        rt.mcp_registry.write().await.disconnect(name).await;
        tracing::info!(
            server = name,
            "disconnected stale web search MCP server for reload"
        );
    }

    let report = startup::connect_web_search_mcp(new_cfg, &rt.mcp_registry).await;
    if !report.failures.is_empty() {
        let failed: Vec<String> = report
            .failures
            .iter()
            .map(|(name, err)| format!("{name}: {err}"))
            .collect();
        publish_notice(
            &rt.publisher,
            format!(
                "web search backend reload failed to connect ({}); web search may be unavailable until this is fixed and residuum is reloaded again",
                failed.join(", ")
            ),
        )
        .await;
    }
}

/// The MCP server name `connect_web_search_mcp` uses for a given standalone
/// backend, or `None` when there's no backend or it names `"ollama"` (a
/// native tool, not an MCP server).
fn web_search_mcp_server_name(
    backend: Option<&crate::config::StandaloneBackendConfig>,
) -> Option<&'static str> {
    match backend.map(|b| b.name.as_str()) {
        Some("brave") => Some("brave_web_search"),
        Some("tavily") => Some("tavily_web_search"),
        _ => None,
    }
}

/// Update observer and reflector thresholds from the new config.
async fn reload_memory_thresholds(rt: &mut GatewayRuntime, new_cfg: &Config) {
    use crate::memory::observer::ObserverConfig;
    use crate::memory::reflector::ReflectorConfig;

    rt.observer.update_config(ObserverConfig {
        threshold_tokens: new_cfg.memory.observer_threshold_tokens,
        cooldown_secs: new_cfg.memory.observer_cooldown_secs,
        force_threshold_tokens: new_cfg.memory.observer_force_threshold_tokens,
        tz: new_cfg.timezone,
        role_overrides: new_cfg.role_overrides.get("observer").cloned(),
    });

    rt.merge_writer
        .update_reflector_config(ReflectorConfig {
            threshold_tokens: new_cfg.memory.reflector_threshold_tokens,
            tz: new_cfg.timezone,
            role_overrides: new_cfg.role_overrides.get("reflector").cloned(),
        })
        .await;

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
                session_registry: std::sync::Arc::clone(&rt.session_registry),
                session_store: std::sync::Arc::clone(&rt.session_store),
                agent_messenger: std::sync::Arc::clone(&rt.agent_messenger),
                skill_state: std::sync::Arc::clone(&rt.skill_state),
                workspace_watch_health: rt.workspace_watch_health.clone(),
                action_store: std::sync::Arc::clone(&rt.action_store),
                layout: rt.layout.clone(),
            };
            let config_api_state = crate::gateway::web::ConfigApiState {
                config_dir: rt.config_dir.clone(),
                workspace_dir: rt.layout.root().to_path_buf(),
                memory_dir: Some(rt.layout.memory_dir()),
                reload_tx: Some(rt.reload_tx.clone()),
                setup_done: None,
                secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
                checkpoints: std::sync::Arc::clone(&rt.checkpoints),
            };
            let update_api_state = crate::gateway::web::update::UpdateApiState {
                update_status: std::sync::Arc::clone(&rt.update_status),
                restart_tx: rt.restart_tx.clone(),
                gateway_shutdown_tx: rt.gateway_shutdown_tx.clone(),
                config_dir: rt.config_dir.clone(),
            };
            let tracing_api_state = crate::gateway::web::tracing_api::TracingApiState {
                service: std::sync::Arc::clone(&rt.tracing_service),
                client_context: std::sync::Arc::new(
                    crate::tracing_service::client_context::gather_for_bug_report(new_cfg),
                ),
                session_registry: std::sync::Arc::clone(&rt.session_registry),
            };
            let memory_api_state = crate::gateway::web::memory::MemoryApiState {
                hybrid_searcher: std::sync::Arc::clone(&rt.hybrid_searcher),
            };
            let model_api_state = crate::gateway::web::model::ModelApiState {
                resources: rt.model_call_resources_tx.subscribe(),
            };
            let a2a_agents_state = crate::gateway::web::a2a::A2aAgentsStatusState {
                hub: std::sync::Arc::clone(&rt.a2a_hub),
            };
            let app = crate::gateway::event_loop::build_gateway_app(
                state,
                config_api_state,
                update_api_state,
                tracing_api_state,
                rt.workbench_serving.clone(),
                crate::gateway::event_loop::ExtraApiStates {
                    memory: memory_api_state,
                    model: model_api_state,
                    a2a_agents: a2a_agents_state,
                },
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
///
/// A directory the rescan couldn't read is already skipped rather than
/// failing the whole rescan (see `SkillIndex::scan`); this surfaces each
/// skip as a notice. Separately, publishes any notice the rescan produced
/// (a skill with an oversized description that loaded anyway, or a skill
/// skipped for invalid frontmatter) so it reaches the user, not just the
/// logs.
async fn reload_skills(rt: &mut GatewayRuntime) {
    let mut skill_guard = rt.skill_state.lock().await;
    if let Err(err) = skill_guard.rescan().await {
        tracing::warn!(error = %err, "skill rescan failed during reload");
        return;
    }
    tracing::debug!("skills rescanned");
    let skipped: Vec<(std::path::PathBuf, String)> = skill_guard.index().skipped_dirs().to_vec();
    let notices: Vec<String> = skill_guard.index().notices().to_vec();
    drop(skill_guard);
    for (dir, err) in skipped {
        publish_notice(
            &rt.publisher,
            format!(
                "Skipped your skills directory \"{}\" — it couldn't be read ({err}). Skills in your other directories were still rescanned.",
                dir.display()
            ),
        )
        .await;
    }
    for notice in notices {
        publish_notice(&rt.publisher, notice).await;
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

/// Update path policy and the main agent's tool-iteration limit from new
/// agent ability gates.
async fn reload_agent_abilities(rt: &mut GatewayRuntime, new_cfg: &Config) {
    rt.path_policy
        .write()
        .await
        .set_blocked_paths(crate::tools::path_policy::blocked_write_paths(
            new_cfg, &rt.layout,
        ));
    rt.agent
        .set_max_tool_iterations(new_cfg.agent.max_tool_iterations);
    rt.agent
        .set_repeat_call_guard(new_cfg.agent.repeat_call_guard);
    tracing::debug!(
        modify_mcp = new_cfg.agent.modify_mcp,
        modify_channels = new_cfg.agent.modify_channels,
        max_tool_iterations = ?new_cfg.agent.max_tool_iterations,
        repeat_call_guard = ?new_cfg.agent.repeat_call_guard,
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
        session_registry: Arc::clone(&rt.session_registry),
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
        let (a2a_port, a2a) = crate::tunnel::a2a_tunnel_params(&new_cfg.a2a);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let status_tx = std::sync::Arc::clone(&rt.tunnel_status_tx);
        let workbench_port = rt.workbench_serving.port();
        rt.tunnel_handle = Some(crate::util::spawn_monitored("tunnel", async move {
            crate::tunnel::start_tunnel(
                cloud,
                workbench_port,
                a2a_port,
                a2a,
                shutdown_rx,
                status_tx,
            )
            .await;
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
        session_registry: Arc::clone(&rt.session_registry),
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
        session_registry: Arc::clone(&rt.session_registry),
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

/// Stop the existing A2A listener (if running) and start a new one if
/// enabled. Unlike the other adapters, a config change also needs a fresh
/// agent card (the base URL or visibility may have changed), so this
/// doesn't go through the generic `reload_adapter` helper.
async fn reload_a2a_adapter(rt: &mut GatewayRuntime, new_cfg: &Config) {
    shutdown_adapter(&mut rt.a2a_shutdown_tx, &mut rt.a2a_handle, "a2a").await;
    rt.a2a_card_state = None;
    rt.a2a_public_url = None;

    if new_cfg.a2a.enabled {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let deps = crate::gateway::event_loop::A2aListenerDeps {
            session_registry: Arc::clone(&rt.session_registry),
            agent_messenger: Arc::clone(&rt.agent_messenger),
            skill_state: Arc::clone(&rt.skill_state),
            bus_handle: rt.bus_handle.clone(),
            tunnel_status_rx: rt.tunnel_status_rx.clone(),
            // The session spawner has been running since startup.
            sessions_ready: tokio::sync::watch::channel(true).1,
        };
        match crate::gateway::event_loop::build_a2a_listener(new_cfg, deps, rx).await {
            Ok((handle, card_state, public_url)) => {
                tracing::info!(
                    visibility = %new_cfg.a2a.visibility,
                    public_url = %public_url.current(),
                    "a2a interface restarted with new config"
                );
                rt.a2a_handle = Some(handle);
                rt.a2a_shutdown_tx = Some(tx);
                rt.a2a_card_state = Some(card_state);
                rt.a2a_public_url = Some(public_url);
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to restart the a2a interface; it will not run until the next successful reload");
            }
        }
    } else {
        tracing::info!("a2a interface removed from config");
    }
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
            a2a: crate::config::A2aConfig::default(),
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
            load_notices: vec![],
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
            context_messages: 20,
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
            context_messages: 20,
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
            context_messages: 20,
        });
        let mut new = old.clone();
        new.telegram = Some(TelegramConfig {
            token: "new-tg-token".to_string(),
            respond_to_others: false,
            context_messages: 20,
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
    fn diff_config_restarts_a2a_on_its_own_changes() {
        let old = test_config();
        let mut new = old.clone();
        new.a2a.visibility = crate::config::A2aVisibility::Private;

        let diff = diff_config(&old, &new);
        assert!(diff.a2a_changed);
        assert!(diff.summary().contains("a2a"));
        assert!(!diff.teams_changed);
    }

    #[test]
    fn diff_config_restarts_a2a_when_the_bind_it_shares_changes() {
        let mut old = test_config();
        old.a2a.enabled = true;
        let mut new = old.clone();
        new.gateway.bind = "0.0.0.0".to_string();
        assert!(diff_config(&old, &new).a2a_changed);

        // Disabled, a bind change is only a gateway change.
        old.a2a.enabled = false;
        new.a2a.enabled = false;
        assert!(!diff_config(&old, &new).a2a_changed);
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
    fn diff_config_detects_web_search_change() {
        let old = test_config();
        let mut new = old.clone();
        new.web_search.standalone_backend = Some(crate::config::StandaloneBackendConfig {
            name: "ollama".to_string(),
            api_key: "key".to_string(),
            base_url: None,
        });

        let diff = diff_config(&old, &new);
        assert!(diff.changed, "web_search-only change should be detected");
        assert!(diff.summary().contains("web search"));
        assert!(
            !diff.summary().contains("providers"),
            "web_search alone should not flag providers"
        );
        assert_no_disruptive_flags(&diff);
    }

    #[test]
    fn web_search_mcp_server_name_maps_known_backends() {
        let backend = |name: &str| {
            Some(crate::config::StandaloneBackendConfig {
                name: name.to_string(),
                api_key: "key".to_string(),
                base_url: None,
            })
        };

        assert_eq!(
            web_search_mcp_server_name(backend("brave").as_ref()),
            Some("brave_web_search")
        );
        assert_eq!(
            web_search_mcp_server_name(backend("tavily").as_ref()),
            Some("tavily_web_search")
        );
        assert_eq!(
            web_search_mcp_server_name(backend("ollama").as_ref()),
            None,
            "ollama is a native tool, not an MCP server"
        );
        assert_eq!(web_search_mcp_server_name(None), None);
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

    fn fireworks_spec(api_key: &str) -> ProviderSpec {
        ProviderSpec {
            name: "fireworks".to_string(),
            model: crate::config::ModelSpec {
                kind: crate::config::ProviderKind::Fireworks,
                model: "accounts/fireworks/models/some-model".to_string(),
            },
            provider_url: "https://api.fireworks.ai".to_string(),
            api_key: Some(api_key.to_string()),
            keep_alive: None,
            session_affinity: None,
        }
    }

    #[test]
    fn diff_config_credential_only_change_names_the_provider() {
        let mut old = test_config();
        old.main = vec![fireworks_spec("real-key")];
        let mut new = old.clone();
        // Simulates a settings save corrupting the credential into an
        // unexpanded env reference: same provider/model/url, different key.
        new.main = vec![fireworks_spec("${FIREWORKS_API_KEY}")];

        let diff = diff_config(&old, &new);

        assert!(
            diff.changed,
            "a credential-only change must still flip `changed`"
        );
        assert!(
            diff.summary().contains("providers"),
            "the generic per-role label should still fire"
        );
        assert!(
            diff.summary()
                .contains("credential changed for fireworks (main)"),
            "summary should name the provider whose credential changed: {}",
            diff.summary()
        );
        assert!(
            !diff.summary().contains("real-key") && !diff.summary().contains("FIREWORKS_API_KEY"),
            "summary must never contain credential values: {}",
            diff.summary()
        );
    }

    #[test]
    fn diff_config_credential_change_ignored_across_resized_chain() {
        let mut old = test_config();
        old.main = vec![fireworks_spec("real-key")];
        let mut new = old.clone();
        new.main = vec![fireworks_spec("real-key"), fireworks_spec("second-key")];

        let diff = diff_config(&old, &new);

        assert!(diff.changed, "adding a failover provider is still a change");
        assert!(
            !diff.summary().contains("credential changed for"),
            "a resized chain is a structural change, not attributable to one provider's credential: {}",
            diff.summary()
        );
    }

    #[test]
    fn diff_config_no_credential_change_when_keys_match() {
        let mut old = test_config();
        old.main = vec![fireworks_spec("same-key")];
        let new = old.clone();

        let diff = diff_config(&old, &new);
        assert!(!diff.changed);
        assert!(!diff.summary().contains("credential changed for"));
    }
}
