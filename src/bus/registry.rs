//! Thread-safe runtime catalog of configured I/O endpoints.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::config::Config;
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

/// Thread-safe, cheaply cloneable catalog of all configured I/O endpoints.
#[derive(Debug, Clone)]
pub struct EndpointRegistry {
    inner: Arc<RwLock<HashMap<EndpointId, EndpointEntry>>>,
}

impl Default for EndpointRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EndpointRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build a registry from the runtime config and external channel definitions.
    #[must_use]
    pub fn from_config(config: &Config, channels: &[ExternalChannelConfig]) -> Self {
        let registry = Self::new();

        // WebSocket — always present
        registry.register(EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Endpoint(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::INTERACTIVE.union(EndpointCapabilities::STREAMING),
            display_name: "WebSocket".to_string(),
        });

        // Discord — if configured
        if config.discord.is_some() {
            registry.register(EndpointEntry {
                id: EndpointId::from("discord"),
                topic: TopicId::Endpoint(EndpointName::from("discord")),
                capabilities: EndpointCapabilities::INTERACTIVE,
                display_name: "Discord".to_string(),
            });
        }

        // Telegram — if configured
        if config.telegram.is_some() {
            registry.register(EndpointEntry {
                id: EndpointId::from("telegram"),
                topic: TopicId::Endpoint(EndpointName::from("telegram")),
                capabilities: EndpointCapabilities::INTERACTIVE,
                display_name: "Telegram".to_string(),
            });
        }

        // External notification channels
        for ch in channels {
            let kind_label = match &ch.kind {
                ExternalChannelKind::Ntfy { .. } => "Ntfy",
                ExternalChannelKind::Webhook { .. } => "Webhook",
                ExternalChannelKind::Macos { .. } => "macOS",
                ExternalChannelKind::Windows { .. } => "Windows",
            };
            registry.register(EndpointEntry {
                id: EndpointId::from(ch.name.as_str()),
                topic: TopicId::Notification(NotifyName::from(ch.name.as_str())),
                capabilities: EndpointCapabilities::NOTIFY_ONLY,
                display_name: format!("{kind_label} ({name})", name = ch.name),
            });
        }

        // Named webhooks
        for name in config.webhooks.keys() {
            registry.register(EndpointEntry {
                id: EndpointId::from(format!("webhook:{name}")),
                topic: TopicId::Inbox,
                capabilities: EndpointCapabilities::INPUT_ONLY,
                display_name: format!("Webhook ({name})"),
            });
        }

        // Inbox — always present
        registry.register(EndpointEntry {
            id: EndpointId::from("inbox"),
            topic: TopicId::Inbox,
            capabilities: EndpointCapabilities::INPUT_ONLY,
            display_name: "Inbox".to_string(),
        });

        registry
    }

    fn read_map(&self) -> std::sync::RwLockReadGuard<'_, HashMap<EndpointId, EndpointEntry>> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_map(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<EndpointId, EndpointEntry>> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Add or overwrite an endpoint entry.
    pub fn register(&self, entry: EndpointEntry) {
        self.write_map().insert(entry.id.clone(), entry);
    }

    /// Remove an endpoint, returning the entry if it existed.
    #[must_use]
    pub fn unregister(&self, id: &EndpointId) -> Option<EndpointEntry> {
        self.write_map().remove(id)
    }

    /// Look up an endpoint by its ID.
    #[must_use]
    pub fn get(&self, id: &EndpointId) -> Option<EndpointEntry> {
        self.read_map().get(id).cloned()
    }

    /// Look up an endpoint by its topic.
    #[must_use]
    pub fn get_by_topic(&self, topic: &TopicId) -> Option<EndpointEntry> {
        self.read_map()
            .values()
            .find(|e| e.topic == *topic)
            .cloned()
    }

    /// Return all endpoints whose capabilities contain all flags in `caps`.
    #[must_use]
    pub fn filter_by(&self, caps: EndpointCapabilities) -> Vec<EndpointEntry> {
        self.read_map()
            .values()
            .filter(|e| e.capabilities.contains(caps))
            .cloned()
            .collect()
    }

    /// Convenience: all interactive endpoints.
    #[must_use]
    pub fn interactive(&self) -> Vec<EndpointEntry> {
        self.filter_by(EndpointCapabilities::INTERACTIVE)
    }

    /// Convenience: all notify-only endpoints.
    #[must_use]
    pub fn notify(&self) -> Vec<EndpointEntry> {
        self.filter_by(EndpointCapabilities::NOTIFY_ONLY)
    }

    /// Return all registered endpoints.
    #[must_use]
    pub fn all(&self) -> Vec<EndpointEntry> {
        self.read_map().values().cloned().collect()
    }

    /// Number of registered endpoints.
    #[must_use]
    pub fn len(&self) -> usize {
        self.read_map().len()
    }

    /// Whether the registry has no endpoints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read_map().is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::indexing_slicing, reason = "test assertions")]
#[expect(clippy::default_trait_access, reason = "test code")]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::config::{
        BackgroundConfig, GatewayConfig, IdleConfig, MemoryConfig, SkillsConfig, WebSearchConfig,
    };
    use crate::models::retry::RetryConfig;

    /// Minimal config for testing.
    fn minimal_config() -> Config {
        Config {
            name: None,
            main: vec![],
            observer: vec![],
            reflector: vec![],
            pulse: vec![],
            embedding: None,
            workspace_dir: PathBuf::from("/tmp"),
            timeout_secs: 30,
            max_tokens: 4096,
            memory: MemoryConfig::default(),
            pulse_enabled: false,
            gateway: GatewayConfig::default(),
            timezone: chrono_tz::UTC,
            cloud: None,
            discord: None,
            telegram: None,
            webhooks: HashMap::new(),
            skills: SkillsConfig { dirs: vec![] },
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
    fn new_creates_empty_registry() {
        let reg = EndpointRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.all().is_empty());
    }

    #[test]
    fn register_and_get() {
        let reg = EndpointRegistry::new();
        let entry = make_entry("ws", EndpointCapabilities::INTERACTIVE);
        reg.register(entry.clone());

        let got = reg.get(&EndpointId::from("ws")).unwrap();
        assert_eq!(got.id, entry.id);
        assert_eq!(got.capabilities, entry.capabilities);
        assert_eq!(got.display_name, entry.display_name);
    }

    #[test]
    fn register_overwrites_existing() {
        let reg = EndpointRegistry::new();
        reg.register(EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Endpoint(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "first".to_string(),
        });
        reg.register(EndpointEntry {
            id: EndpointId::from("ws"),
            topic: TopicId::Endpoint(EndpointName::from("ws")),
            capabilities: EndpointCapabilities::STREAMING,
            display_name: "second".to_string(),
        });

        assert_eq!(reg.len(), 1);
        let got = reg.get(&EndpointId::from("ws")).unwrap();
        assert_eq!(got.display_name, "second");
        assert_eq!(got.capabilities, EndpointCapabilities::STREAMING);
    }

    #[test]
    fn unregister_removes_entry() {
        let reg = EndpointRegistry::new();
        reg.register(make_entry("ws", EndpointCapabilities::INTERACTIVE));

        let removed = reg.unregister(&EndpointId::from("ws"));
        assert!(removed.is_some());
        assert!(reg.is_empty());
        assert!(reg.get(&EndpointId::from("ws")).is_none());
    }

    #[test]
    fn unregister_nonexistent_returns_none() {
        let reg = EndpointRegistry::new();
        assert!(reg.unregister(&EndpointId::from("nope")).is_none());
    }

    #[test]
    fn get_by_topic() {
        let reg = EndpointRegistry::new();
        let entry = EndpointEntry {
            id: EndpointId::from("telegram"),
            topic: TopicId::Endpoint(EndpointName::from("telegram")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "Telegram".to_string(),
        };
        reg.register(entry);

        let got = reg
            .get_by_topic(&TopicId::Endpoint(EndpointName::from("telegram")))
            .unwrap();
        assert_eq!(got.id, EndpointId::from("telegram"));
    }

    #[test]
    fn get_by_topic_not_found() {
        let reg = EndpointRegistry::new();
        reg.register(make_entry("ws", EndpointCapabilities::INTERACTIVE));
        assert!(
            reg.get_by_topic(&TopicId::Endpoint(EndpointName::from("missing")))
                .is_none()
        );
    }

    #[test]
    fn filter_by_interactive() {
        let reg = EndpointRegistry::new();
        reg.register(make_entry(
            "ws",
            EndpointCapabilities::INTERACTIVE.union(EndpointCapabilities::STREAMING),
        ));
        reg.register(EndpointEntry {
            id: EndpointId::from("ntfy"),
            topic: TopicId::Notification(NotifyName::from("ntfy")),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: "ntfy".to_string(),
        });
        reg.register(EndpointEntry {
            id: EndpointId::from("inbox"),
            topic: TopicId::Inbox,
            capabilities: EndpointCapabilities::INPUT_ONLY,
            display_name: "Inbox".to_string(),
        });

        let interactive = reg.filter_by(EndpointCapabilities::INTERACTIVE);
        assert_eq!(interactive.len(), 1);
        assert_eq!(interactive[0].id, EndpointId::from("ws"));
    }

    #[test]
    fn filter_by_notify_only() {
        let reg = EndpointRegistry::new();
        reg.register(make_entry("ws", EndpointCapabilities::INTERACTIVE));
        reg.register(EndpointEntry {
            id: EndpointId::from("ntfy"),
            topic: TopicId::Notification(NotifyName::from("ntfy")),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: "ntfy".to_string(),
        });

        let notify = reg.filter_by(EndpointCapabilities::NOTIFY_ONLY);
        assert_eq!(notify.len(), 1);
        assert_eq!(notify[0].id, EndpointId::from("ntfy"));
    }

    #[test]
    fn interactive_convenience() {
        let reg = EndpointRegistry::new();
        reg.register(make_entry("ws", EndpointCapabilities::INTERACTIVE));
        reg.register(EndpointEntry {
            id: EndpointId::from("ntfy"),
            topic: TopicId::Notification(NotifyName::from("ntfy")),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: "ntfy".to_string(),
        });

        let result = reg.interactive();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, EndpointId::from("ws"));
    }

    #[test]
    fn notify_convenience() {
        let reg = EndpointRegistry::new();
        reg.register(make_entry("ws", EndpointCapabilities::INTERACTIVE));
        reg.register(EndpointEntry {
            id: EndpointId::from("ntfy"),
            topic: TopicId::Notification(NotifyName::from("ntfy")),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: "ntfy".to_string(),
        });

        let result = reg.notify();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, EndpointId::from("ntfy"));
    }

    #[test]
    fn all_returns_everything() {
        let reg = EndpointRegistry::new();
        reg.register(make_entry("ws", EndpointCapabilities::INTERACTIVE));
        reg.register(make_entry("discord", EndpointCapabilities::INTERACTIVE));
        reg.register(EndpointEntry {
            id: EndpointId::from("ntfy"),
            topic: TopicId::Notification(NotifyName::from("ntfy")),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: "ntfy".to_string(),
        });

        assert_eq!(reg.all().len(), 3);
    }

    #[test]
    fn from_config_ws_always_present() {
        let config = minimal_config();
        let reg = EndpointRegistry::from_config(&config, &[]);

        // ws and inbox are always present
        let ws = reg.get(&EndpointId::from("ws")).unwrap();
        assert_eq!(ws.display_name, "WebSocket");
        assert!(ws.capabilities.contains(EndpointCapabilities::INTERACTIVE));
        assert!(ws.capabilities.contains(EndpointCapabilities::STREAMING));

        let inbox = reg.get(&EndpointId::from("inbox")).unwrap();
        assert_eq!(inbox.display_name, "Inbox");
        assert!(
            inbox
                .capabilities
                .contains(EndpointCapabilities::INPUT_ONLY)
        );

        // discord/telegram not present
        assert!(reg.get(&EndpointId::from("discord")).is_none());
        assert!(reg.get(&EndpointId::from("telegram")).is_none());
        assert!(reg.get(&EndpointId::from("webhook")).is_none());
    }

    #[test]
    fn from_config_includes_discord_when_configured() {
        let mut config = minimal_config();
        config.discord = Some(crate::config::DiscordConfig {
            token: "test-token".to_string(),
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

    #[test]
    fn from_config_empty_webhooks() {
        let config = minimal_config();
        let reg = EndpointRegistry::from_config(&config, &[]);
        assert_eq!(reg.len(), 2); // ws + inbox only
    }

    #[test]
    fn from_config_named_webhooks() {
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
        config.webhooks.insert(
            "deploy".to_string(),
            crate::config::WebhookEntry {
                secret: Some("tok".to_string()),
                routing: crate::config::WebhookRouting::Agent("deployer".to_string()),
                format: crate::config::WebhookFormat::Raw,
                content_fields: None,
            },
        );

        let reg = EndpointRegistry::from_config(&config, &[]);
        let gh = reg.get(&EndpointId::from("webhook:github")).unwrap();
        assert_eq!(gh.display_name, "Webhook (github)");
        assert!(gh.capabilities.contains(EndpointCapabilities::INPUT_ONLY));
        assert_eq!(gh.topic, TopicId::Inbox);

        let deploy = reg.get(&EndpointId::from("webhook:deploy")).unwrap();
        assert_eq!(deploy.display_name, "Webhook (deploy)");
    }

    #[test]
    fn clone_shares_state() {
        let reg = EndpointRegistry::new();
        let reg2 = reg.clone();

        reg.register(make_entry("ws", EndpointCapabilities::INTERACTIVE));

        // clone sees the same state
        assert_eq!(reg2.len(), 1);
        assert!(reg2.get(&EndpointId::from("ws")).is_some());
    }
}
