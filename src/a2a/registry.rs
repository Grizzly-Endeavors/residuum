//! Registry of known remote A2A agents.
//!
//! Tracks agents available for task delegation, populated from config
//! at startup and from runtime discovery via `a2a_discover`.

use std::sync::Arc;

use tokio::sync::Mutex;

use super::types::AgentCard;

/// Shared handle to the A2A agent registry.
pub type SharedA2aRegistry = Arc<Mutex<A2aRegistry>>;

/// A known remote A2A agent.
#[derive(Debug, Clone)]
pub struct RemoteAgent {
    /// Logical name for this agent.
    pub name: String,
    /// Base URL of the remote agent.
    pub url: String,
    /// Optional bearer token for authentication.
    pub secret: Option<String>,
    /// Discovered Agent Card (None if not yet discovered).
    pub card: Option<AgentCard>,
}

/// In-memory registry of known remote A2A agents.
#[derive(Debug)]
pub struct A2aRegistry {
    agents: Vec<RemoteAgent>,
}

impl Default for A2aRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl A2aRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self { agents: vec![] }
    }

    /// Build a registry from config, with agent cards initially unset.
    #[must_use]
    pub fn from_config(agents: &[crate::config::RemoteAgentConfig]) -> Self {
        let agents = agents
            .iter()
            .map(|cfg| RemoteAgent {
                name: cfg.name.clone(),
                url: cfg.url.clone(),
                secret: cfg.secret.clone(),
                card: None,
            })
            .collect();
        Self { agents }
    }

    /// Create an empty shared registry.
    #[must_use]
    pub fn new_shared() -> SharedA2aRegistry {
        Arc::new(Mutex::new(Self::new()))
    }

    /// Build a shared registry from config.
    #[must_use]
    pub fn from_config_shared(agents: &[crate::config::RemoteAgentConfig]) -> SharedA2aRegistry {
        Arc::new(Mutex::new(Self::from_config(agents)))
    }

    /// Add an agent, or update the existing entry if one with the same name exists.
    pub fn add(&mut self, agent: RemoteAgent) {
        if let Some(existing) = self.agents.iter_mut().find(|a| a.name == agent.name) {
            *existing = agent;
        } else {
            self.agents.push(agent);
        }
    }

    /// Remove an agent by name, returning whether it was found.
    pub fn remove(&mut self, name: &str) -> bool {
        let len_before = self.agents.len();
        self.agents.retain(|a| a.name != name);
        self.agents.len() < len_before
    }

    /// Find an agent by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&RemoteAgent> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// Return all registered agents.
    #[must_use]
    pub fn list(&self) -> &[RemoteAgent] {
        &self.agents
    }

    /// Set the discovered agent card for a named agent.
    ///
    /// Does nothing if no agent with the given name exists.
    pub fn update_card(&mut self, name: &str, card: AgentCard) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.name == name) {
            agent.card = Some(card);
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::config::RemoteAgentConfig;

    fn sample_config() -> Vec<RemoteAgentConfig> {
        vec![
            RemoteAgentConfig {
                name: "alpha".to_string(),
                url: "http://alpha:8080".to_string(),
                secret: Some("tok-alpha".to_string()),
            },
            RemoteAgentConfig {
                name: "beta".to_string(),
                url: "http://beta:9090".to_string(),
                secret: None,
            },
        ]
    }

    fn sample_card(name: &str) -> AgentCard {
        AgentCard {
            name: name.to_string(),
            description: format!("{name} agent"),
            url: format!("http://{name}:8080/a2a"),
            version: "0.2".to_string(),
            capabilities: None,
            skills: vec![],
            default_input_modes: vec!["text/plain".to_string()],
            default_output_modes: vec!["text/plain".to_string()],
            authentication: None,
        }
    }

    #[test]
    fn from_config_creates_agents_without_cards() {
        let registry = A2aRegistry::from_config(&sample_config());

        assert_eq!(registry.list().len(), 2);
        let alpha = registry.get("alpha").unwrap();
        assert_eq!(alpha.url, "http://alpha:8080");
        assert_eq!(alpha.secret.as_deref(), Some("tok-alpha"));
        assert!(alpha.card.is_none());

        let beta = registry.get("beta").unwrap();
        assert_eq!(beta.url, "http://beta:9090");
        assert!(beta.secret.is_none());
        assert!(beta.card.is_none());
    }

    #[test]
    fn add_and_get() {
        let mut registry = A2aRegistry::new();
        assert!(registry.get("gamma").is_none());

        registry.add(RemoteAgent {
            name: "gamma".to_string(),
            url: "http://gamma:7000".to_string(),
            secret: None,
            card: None,
        });

        let agent = registry.get("gamma").unwrap();
        assert_eq!(agent.url, "http://gamma:7000");
    }

    #[test]
    fn add_updates_existing_agent_by_name() {
        let mut registry = A2aRegistry::from_config(&sample_config());

        registry.add(RemoteAgent {
            name: "alpha".to_string(),
            url: "http://alpha-v2:8080".to_string(),
            secret: Some("new-tok".to_string()),
            card: None,
        });

        assert_eq!(registry.list().len(), 2, "should not duplicate");
        let alpha = registry.get("alpha").unwrap();
        assert_eq!(alpha.url, "http://alpha-v2:8080");
        assert_eq!(alpha.secret.as_deref(), Some("new-tok"));
    }

    #[test]
    fn remove_returns_true_when_found() {
        let mut registry = A2aRegistry::from_config(&sample_config());
        assert!(registry.remove("alpha"));
        assert!(registry.get("alpha").is_none());
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn remove_returns_false_when_not_found() {
        let mut registry = A2aRegistry::from_config(&sample_config());
        assert!(!registry.remove("nonexistent"));
        assert_eq!(registry.list().len(), 2);
    }

    #[test]
    fn update_card_sets_the_card() {
        let mut registry = A2aRegistry::from_config(&sample_config());
        let card = sample_card("alpha");

        registry.update_card("alpha", card);

        let alpha = registry.get("alpha").unwrap();
        let card = alpha.card.as_ref().unwrap();
        assert_eq!(card.name, "alpha");
        assert_eq!(card.version, "0.2");
    }

    #[test]
    fn update_card_ignores_unknown_agent() {
        let mut registry = A2aRegistry::from_config(&sample_config());
        // should not panic or add an entry
        registry.update_card("nonexistent", sample_card("nonexistent"));
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn list_returns_all_agents() {
        let registry = A2aRegistry::from_config(&sample_config());
        let all = registry.list();
        assert_eq!(all.len(), 2);

        let names: Vec<&str> = all.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }
}
