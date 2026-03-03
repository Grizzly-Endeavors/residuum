//! In-place root config reload: diff old vs new config and update changed subsystems.

use std::sync::Arc;

use crate::config::Config;
use crate::gateway::protocol::ServerMessage;
use crate::models::CompletionOptions;

use super::GatewayRuntime;
use super::spawn_helpers::SpawnContext;

/// Which subsystems differ between two `Config` snapshots.
#[expect(
    clippy::struct_excessive_bools,
    reason = "diff struct deliberately uses bool flags for each subsystem"
)]
pub(super) struct ConfigDiff {
    /// Provider chains changed (main, observer, reflector, pulse, embedding, retry, `max_tokens`).
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
}

impl ConfigDiff {
    /// Returns `true` if nothing changed between old and new config.
    fn is_empty(&self) -> bool {
        !self.providers_changed
            && !self.memory_changed
            && !self.gateway_changed
            && !self.discord_changed
            && !self.telegram_changed
            && !self.pulse_changed
            && !self.background_changed
            && !self.agent_changed
            && !self.skills_changed
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
            || old.max_tokens != new.max_tokens,
        memory_changed: old.memory != new.memory,
        gateway_changed: old.gateway != new.gateway,
        discord_changed: old.discord != new.discord,
        telegram_changed: old.telegram != new.telegram,
        pulse_changed: old.pulse_enabled != new.pulse_enabled,
        background_changed: old.background != new.background,
        agent_changed: old.agent != new.agent,
        skills_changed: old.skills != new.skills,
    }
}

/// Handle an in-place root config reload.
///
/// Backs up current config files, loads new config, diffs old vs new, and
/// applies only the changed subsystems. On failure, rolls back and notifies
/// clients.
#[expect(
    clippy::too_many_lines,
    reason = "linear pipeline applying each diff field in sequence"
)]
pub(super) async fn handle_root_reload(rt: &mut GatewayRuntime) {
    tracing::info!("handling root config reload in-place");
    super::backup_config(&rt.config_dir);

    let new_cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(error = %err, "config reload failed, keeping current config");
            super::rollback_config(&rt.config_dir);
            rt.broadcast_tx
                .send(ServerMessage::Notice {
                    message: format!("config reload failed (keeping current config): {err}"),
                })
                .ok();
            return;
        }
    };

    let diff = diff_config(&rt.cfg, &new_cfg);

    if diff.is_empty() {
        rt.broadcast_tx
            .send(ServerMessage::Notice {
                message: "configuration reloaded: no changes detected".to_string(),
            })
            .ok();
        tracing::info!("config reload: no changes detected");
        return;
    }

    let summary = diff.summary();

    // ── Provider swap ───────────────────────────────────────────────────
    if diff.providers_changed {
        match super::startup::init_providers(&new_cfg, rt.tz, rt.http_client.clone()) {
            Ok(components) => {
                rt.agent.swap_provider(components.provider);
                rt.observer = components.observer;
                rt.reflector = components.reflector;
                rt.embedding_provider = components.embedding_provider;

                // Rebuild SpawnContext with new provider specs
                let spawn_ctx = Arc::new(SpawnContext {
                    background_config: new_cfg.background.clone(),
                    main_provider_specs: new_cfg.main.clone(),
                    http_client: rt.http_client.clone(),
                    max_tokens: new_cfg.max_tokens,
                    retry_config: new_cfg.retry.clone(),
                    identity: rt.spawn_context.identity.clone(),
                    options: CompletionOptions {
                        max_tokens: Some(new_cfg.max_tokens),
                        ..CompletionOptions::default()
                    },
                    layout: rt.layout.clone(),
                    tz: rt.tz,
                });
                rt.spawn_context = spawn_ctx;

                tracing::info!("providers swapped successfully");
            }
            Err(err) => {
                tracing::warn!(error = %err, "provider rebuild failed, keeping current providers");
                rt.broadcast_tx
                    .send(ServerMessage::Notice {
                        message: format!("provider rebuild failed (keeping current): {err}"),
                    })
                    .ok();
            }
        }
    }

    // ── Memory thresholds ───────────────────────────────────────────────
    if diff.memory_changed {
        use crate::memory::observer::ObserverConfig;
        use crate::memory::reflector::ReflectorConfig;

        rt.observer.update_config(ObserverConfig {
            threshold_tokens: new_cfg.memory.observer_threshold_tokens,
            cooldown_secs: new_cfg.memory.observer_cooldown_secs,
            force_threshold_tokens: new_cfg.memory.observer_force_threshold_tokens,
            tz: new_cfg.timezone,
        });

        rt.reflector.update_config(ReflectorConfig {
            threshold_tokens: new_cfg.memory.reflector_threshold_tokens,
            tz: new_cfg.timezone,
        });

        tracing::info!("memory thresholds updated");
    }

    // ── Gateway bind/port (deferred to Phase 6) ─────────────────────────
    if diff.gateway_changed {
        rt.broadcast_tx
            .send(ServerMessage::Notice {
                message: "gateway bind/port changed — restart required to take effect".to_string(),
            })
            .ok();
        tracing::warn!("gateway bind/port changed, restart required");
    }

    // ── Discord token (deferred to Phase 6) ─────────────────────────────
    if diff.discord_changed {
        rt.broadcast_tx
            .send(ServerMessage::Notice {
                message: "discord token changed — restart required to take effect".to_string(),
            })
            .ok();
        tracing::warn!("discord token changed, restart required");
    }

    // ── Telegram token (deferred to Phase 6) ────────────────────────────
    if diff.telegram_changed {
        rt.broadcast_tx
            .send(ServerMessage::Notice {
                message: "telegram token changed — restart required to take effect".to_string(),
            })
            .ok();
        tracing::warn!("telegram token changed, restart required");
    }

    // ── Pulse toggle ────────────────────────────────────────────────────
    if diff.pulse_changed {
        rt.pulse_enabled = new_cfg.pulse_enabled;
        tracing::info!(enabled = new_cfg.pulse_enabled, "pulse toggle updated");
    }

    // ── Background config ───────────────────────────────────────────────
    if diff.background_changed && !diff.providers_changed {
        // If providers also changed, SpawnContext was already rebuilt above.
        let spawn_ctx = Arc::new(SpawnContext {
            background_config: new_cfg.background.clone(),
            main_provider_specs: new_cfg.main.clone(),
            http_client: rt.http_client.clone(),
            max_tokens: new_cfg.max_tokens,
            retry_config: new_cfg.retry.clone(),
            identity: rt.spawn_context.identity.clone(),
            options: CompletionOptions {
                max_tokens: Some(new_cfg.max_tokens),
                ..CompletionOptions::default()
            },
            layout: rt.layout.clone(),
            tz: rt.tz,
        });
        rt.spawn_context = spawn_ctx;
        tracing::info!("background config updated");
    }

    // ── Skills rescan ───────────────────────────────────────────────────
    if diff.skills_changed {
        let mut skill_guard = rt.skill_state.lock().await;
        // Rescan with no project-specific skills dir (project rescan happens separately)
        if let Err(err) = skill_guard.rescan(None).await {
            tracing::warn!(error = %err, "skill rescan failed during reload");
        } else {
            tracing::info!("skills rescanned");
        }
    }

    // ── Store new config ────────────────────────────────────────────────
    rt.cfg = new_cfg;

    rt.broadcast_tx
        .send(ServerMessage::Notice {
            message: format!("configuration reloaded: {summary}"),
        })
        .ok();
    tracing::info!(changes = %summary, "configuration reloaded successfully");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentAbilitiesConfig, BackgroundConfig, DiscordConfig, GatewayConfig, MemoryConfig,
        SkillsConfig, TelegramConfig, WebhookConfig,
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
            discord: None,
            telegram: None,
            webhook: WebhookConfig::default(),
            skills: SkillsConfig { dirs: vec![] },
            retry: RetryConfig::default(),
            background: BackgroundConfig::default(),
            agent: AgentAbilitiesConfig::default(),
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

        let diff = diff_config(&old, &new);
        assert!(diff.providers_changed);
        assert!(diff.memory_changed);
        assert!(diff.pulse_changed);
        assert!(diff.discord_changed);
        assert!(diff.telegram_changed);
        assert!(diff.skills_changed);
        assert!(diff.agent_changed);
        assert!(diff.background_changed);
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
    }
}
