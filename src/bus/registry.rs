//! Runtime catalog of configured I/O endpoints.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::Config;
use crate::interfaces::conversations::ConversationDirectory;
use crate::notify::types::{ExternalChannelConfig, ExternalChannelKind};

use super::endpoint::EndpointCapabilities;
use super::types::EndpointId;
use super::types::{EndpointName, NotifyName, TopicId};

// ---------------------------------------------------------------------------
// EndpointEntry
// ---------------------------------------------------------------------------

/// A single endpoint registered in the catalog.
#[derive(Debug, Clone)]
pub struct EndpointEntry {
    pub id: EndpointId,
    pub topic: TopicId,
    pub capabilities: EndpointCapabilities,
    pub display_name: String,
}

// ---------------------------------------------------------------------------
// EndpointRegistry
// ---------------------------------------------------------------------------

type EntryMap = HashMap<EndpointId, EndpointEntry>;

/// Shared, cheaply cloneable catalog of all configured I/O endpoints.
///
/// Every clone refers to the same catalog, so a [`refresh`](Self::refresh)
/// after a config or `channels.toml` reload is seen by everything holding
/// one — tools, the notification router, idle switching. It also carries the
/// directory of conversations the running chat interfaces can reach.
#[derive(Debug, Clone, Default)]
pub struct EndpointRegistry {
    entries: Arc<std::sync::RwLock<Arc<EntryMap>>>,
    conversations: ConversationDirectory,
}

fn index_entries(entries: impl IntoIterator<Item = EndpointEntry>) -> EntryMap {
    entries.into_iter().map(|e| (e.id.clone(), e)).collect()
}

impl EndpointRegistry {
    /// Build a registry from a set of entries; a later entry with a duplicate ID wins.
    #[must_use]
    pub fn from_entries(entries: impl IntoIterator<Item = EndpointEntry>) -> Self {
        Self {
            entries: Arc::new(std::sync::RwLock::new(Arc::new(index_entries(entries)))),
            conversations: ConversationDirectory::default(),
        }
    }

    /// Build a registry from the runtime config and external channel definitions.
    #[must_use]
    pub fn from_config(config: &Config, channels: &[ExternalChannelConfig]) -> Self {
        Self::from_entries(Self::config_entries(config, channels))
    }

    /// Replace the catalog for every clone of this registry.
    pub fn refresh(&self, config: &Config, channels: &[ExternalChannelConfig]) {
        let fresh = Arc::new(index_entries(Self::config_entries(config, channels)));
        *self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = fresh;
    }

    /// Conversations each running chat interface can reach.
    #[must_use]
    pub(crate) fn conversations(&self) -> &ConversationDirectory {
        &self.conversations
    }

    /// Snapshot of the current catalog.
    fn snapshot(&self) -> Arc<EntryMap> {
        // The lock only guards an Arc swap; a poisoned lock still holds a whole map.
        Arc::clone(
            &self
                .entries
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn config_entries(config: &Config, channels: &[ExternalChannelConfig]) -> Vec<EndpointEntry> {
        // WebSocket — always present
        let mut entries = vec![EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Endpoint(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::INTERACTIVE.union(EndpointCapabilities::STREAMING),
            display_name: "WebSocket".to_string(),
        }];

        if config.discord.is_some() {
            entries.push(EndpointEntry {
                id: EndpointId::from("discord"),
                topic: TopicId::Endpoint(EndpointName::from("discord")),
                capabilities: EndpointCapabilities::INTERACTIVE,
                display_name: "Discord".to_string(),
            });
        }

        if config.telegram.is_some() {
            entries.push(EndpointEntry {
                id: EndpointId::from("telegram"),
                topic: TopicId::Endpoint(EndpointName::from("telegram")),
                capabilities: EndpointCapabilities::INTERACTIVE,
                display_name: "Telegram".to_string(),
            });
        }

        if config.teams.is_some() {
            entries.push(EndpointEntry {
                id: EndpointId::from(crate::interfaces::teams::ENDPOINT),
                topic: TopicId::Endpoint(EndpointName::from(crate::interfaces::teams::ENDPOINT)),
                capabilities: EndpointCapabilities::INTERACTIVE,
                display_name: "Microsoft Teams".to_string(),
            });
        }

        for ch in channels {
            let kind_label = match &ch.kind {
                ExternalChannelKind::Ntfy { .. } => "Ntfy",
                ExternalChannelKind::Webhook { .. } => "Webhook",
                ExternalChannelKind::Macos { .. } => "macOS",
                ExternalChannelKind::Windows { .. } => "Windows",
            };
            entries.push(EndpointEntry {
                id: EndpointId::from(ch.name.as_str()),
                topic: TopicId::Notification(NotifyName::from(ch.name.as_str())),
                capabilities: EndpointCapabilities::NOTIFY_ONLY,
                display_name: format!("{kind_label} ({name})", name = ch.name),
            });
        }

        entries
    }

    /// Look up an endpoint by its ID.
    #[must_use]
    pub fn get(&self, id: &EndpointId) -> Option<EndpointEntry> {
        self.snapshot().get(id).cloned()
    }

    /// All interactive endpoints.
    #[must_use]
    pub fn interactive(&self) -> Vec<EndpointEntry> {
        self.with_capabilities(EndpointCapabilities::INTERACTIVE)
    }

    /// All notify-only endpoints.
    #[must_use]
    pub fn notify(&self) -> Vec<EndpointEntry> {
        self.with_capabilities(EndpointCapabilities::NOTIFY_ONLY)
    }

    fn with_capabilities(&self, caps: EndpointCapabilities) -> Vec<EndpointEntry> {
        self.snapshot()
            .values()
            .filter(|e| e.capabilities.contains(caps))
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::indexing_slicing, reason = "test assertions")]
#[expect(clippy::default_trait_access, reason = "test code")]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::config::{
        BackgroundConfig, GatewayConfig, IdleConfig, MemoryConfig, SkillsConfig, ToolsConfig,
        WebSearchConfig,
    };
    use crate::inference::retry::RetryConfig;

    /// Minimal config for testing.
    fn minimal_config() -> Config {
        Config {
            name: None,
            main: vec![],
            observer: vec![],
            reflector: vec![],
            pulse: vec![],
            subconscious: vec![],
            embedding: None,
            workspace_dir: PathBuf::from("/tmp"),
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
            webhooks: HashMap::new(),
            skills: SkillsConfig { dirs: vec![] },
            tools: ToolsConfig { dirs: vec![] },
            retry: RetryConfig::default(),
            background: BackgroundConfig::default(),
            agent: Default::default(),
            idle: IdleConfig::default(),
            temperature: None,
            thinking: None,
            web_search: WebSearchConfig::default(),
            tracing: crate::config::TracingConfig::default(),
            role_overrides: HashMap::new(),
            config_dir: PathBuf::from("/tmp"),
        }
    }

    fn make_entry(id: &str, caps: EndpointCapabilities) -> EndpointEntry {
        EndpointEntry {
            id: EndpointId::from(id),
            topic: TopicId::Endpoint(EndpointName::from(id)),
            capabilities: caps,
            display_name: id.to_string(),
        }
    }

    #[test]
    fn default_is_empty() {
        let reg = EndpointRegistry::default();
        assert!(reg.interactive().is_empty());
        assert!(reg.notify().is_empty());
        assert!(reg.get(&EndpointId::from("ws")).is_none());
    }

    #[test]
    fn from_entries_later_duplicate_wins() {
        let reg = EndpointRegistry::from_entries([
            EndpointEntry {
                display_name: "first".to_string(),
                ..make_entry("ws", EndpointCapabilities::INTERACTIVE)
            },
            EndpointEntry {
                display_name: "second".to_string(),
                ..make_entry("ws", EndpointCapabilities::STREAMING)
            },
        ]);

        let got = reg.get(&EndpointId::from("ws")).unwrap();
        assert_eq!(got.display_name, "second");
        assert_eq!(got.capabilities, EndpointCapabilities::STREAMING);
    }

    #[test]
    fn interactive_and_notify_partition_by_capability() {
        let reg = EndpointRegistry::from_entries([
            make_entry(
                "ws",
                EndpointCapabilities::INTERACTIVE.union(EndpointCapabilities::STREAMING),
            ),
            EndpointEntry {
                id: EndpointId::from("ntfy"),
                topic: TopicId::Notification(NotifyName::from("ntfy")),
                capabilities: EndpointCapabilities::NOTIFY_ONLY,
                display_name: "ntfy".to_string(),
            },
        ]);

        let interactive = reg.interactive();
        assert_eq!(interactive.len(), 1);
        assert_eq!(interactive[0].id, EndpointId::from("ws"));

        let notify = reg.notify();
        assert_eq!(notify.len(), 1);
        assert_eq!(notify[0].id, EndpointId::from("ntfy"));
    }

    #[test]
    fn from_config_ignores_webhooks() {
        let mut config = minimal_config();
        config.webhooks.insert(
            "github".to_string(),
            crate::config::WebhookEntry {
                secret: None,
                routing: crate::config::WebhookRouting::Inbox,
                format: crate::config::WebhookFormat::Parsed,
                content_fields: None,
            },
        );

        let reg = EndpointRegistry::from_config(&config, &[]);
        assert!(reg.get(&EndpointId::from("webhook:github")).is_none());
        assert_eq!(reg.interactive().len(), 1);
        assert!(reg.notify().is_empty());
    }

    #[test]
    fn from_config_ws_always_present() {
        let config = minimal_config();
        let reg = EndpointRegistry::from_config(&config, &[]);

        let ws = reg.get(&EndpointId::from("ws")).unwrap();
        assert_eq!(ws.display_name, "WebSocket");
        assert!(ws.capabilities.contains(EndpointCapabilities::INTERACTIVE));
        assert!(ws.capabilities.contains(EndpointCapabilities::STREAMING));

        // discord/telegram not present
        assert!(reg.get(&EndpointId::from("discord")).is_none());
        assert!(reg.get(&EndpointId::from("telegram")).is_none());
    }

    #[test]
    fn from_config_includes_discord_when_configured() {
        let mut config = minimal_config();
        config.discord = Some(crate::config::DiscordConfig {
            token: "test-token".to_string(),
            respond_to_others: false,
            context_messages: 20,
        });

        let reg = EndpointRegistry::from_config(&config, &[]);

        let discord = reg.get(&EndpointId::from("discord")).unwrap();
        assert_eq!(discord.display_name, "Discord");
        assert!(
            discord
                .capabilities
                .contains(EndpointCapabilities::INTERACTIVE)
        );
    }

    #[test]
    fn refresh_is_seen_by_every_clone() {
        let mut config = minimal_config();
        let registry = EndpointRegistry::from_config(&config, &[]);
        let held_by_a_tool = registry.clone();
        assert!(held_by_a_tool.get(&EndpointId::from("discord")).is_none());

        config.discord = Some(crate::config::DiscordConfig {
            token: "added-on-reload".to_string(),
            respond_to_others: false,
            context_messages: 20,
        });
        registry.refresh(&config, &[]);
        assert!(
            held_by_a_tool.get(&EndpointId::from("discord")).is_some(),
            "an adapter added by a reload is visible without a restart"
        );

        config.discord = None;
        registry.refresh(&config, &[]);
        assert!(held_by_a_tool.get(&EndpointId::from("discord")).is_none());
        assert!(
            held_by_a_tool.get(&EndpointId::from("ws")).is_some(),
            "ws survives every refresh"
        );
    }

    #[test]
    fn from_config_includes_teams_when_configured() {
        let mut config = minimal_config();
        assert!(reg_has_teams(&config).is_none());

        config.teams = Some(crate::config::TeamsConfig {
            app_id: "app".to_string(),
            app_password: "secret".to_string(),
            tenant_id: "tenant".to_string(),
            respond_to_others: false,
            context_messages: 20,
            port: 7701,
        });
        let teams = reg_has_teams(&config).unwrap();
        assert_eq!(teams.display_name, "Microsoft Teams");
        assert!(
            teams
                .capabilities
                .contains(EndpointCapabilities::INTERACTIVE)
        );
    }

    fn reg_has_teams(config: &Config) -> Option<EndpointEntry> {
        EndpointRegistry::from_config(config, &[]).get(&EndpointId::from("teams"))
    }

    #[test]
    fn from_config_includes_external_channels() {
        let config = minimal_config();
        let channels = vec![ExternalChannelConfig {
            name: "my-ntfy".to_string(),
            kind: ExternalChannelKind::Ntfy {
                url: "https://ntfy.sh".to_string(),
                topic: "test".to_string(),
                priority: None,
            },
        }];

        let reg = EndpointRegistry::from_config(&config, &channels);

        let ch = reg.get(&EndpointId::from("my-ntfy")).unwrap();
        assert_eq!(ch.display_name, "Ntfy (my-ntfy)");
        assert!(ch.capabilities.contains(EndpointCapabilities::NOTIFY_ONLY));
        assert_eq!(ch.topic, TopicId::Notification(NotifyName::from("my-ntfy")));
    }
}
