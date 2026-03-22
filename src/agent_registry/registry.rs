//! Agent registry — tracks named agents and their assigned ports.
//!
//! The registry is a TOML file at `~/.residuum/agent_registry/registry.toml`
//! containing a list of `[[agents]]` entries with name and port.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{trace, warn};

use crate::error::FatalError;

/// Starting port for named agents. Default agent uses 7700.
const AGENT_PORT_START: u16 = 7701;

/// Warn when port scanning advances further than this many steps from the starting port.
const MAX_EXPECTED_AGENTS: u16 = 20;

/// A single agent entry in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    pub name: String,
    pub port: u16,
}

/// The agent registry file structure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentRegistry {
    #[serde(default)]
    agents: Vec<AgentEntry>,
}

impl AgentRegistry {
    /// Load the registry from `base_dir/registry.toml`.
    ///
    /// Returns an empty registry if the file doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns `FatalError::Config` if the file exists but cannot be read or parsed.
    pub fn load(base_dir: &Path) -> Result<Self, FatalError> {
        let path = base_dir.join("registry.toml");
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(&path).map_err(|e| {
            FatalError::Config(format!(
                "failed to read agent registry at {}: {e}",
                path.display()
            ))
        })?;

        let registry: Self = toml::from_str(&contents).map_err(|e| {
            FatalError::Config(format!(
                "failed to parse agent registry at {}: {e}",
                path.display()
            ))
        })?;
        trace!(path = %path.display(), agents = registry.agents.len(), "loaded agent registry");
        Ok(registry)
    }

    /// Save the registry to `base_dir/registry.toml`.
    ///
    /// Creates the directory if it doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns `FatalError::Config` if the file cannot be written.
    pub fn save(&self, base_dir: &Path) -> Result<(), FatalError> {
        std::fs::create_dir_all(base_dir).map_err(|e| {
            FatalError::Config(format!(
                "failed to create agent registry directory {}: {e}",
                base_dir.display()
            ))
        })?;

        let contents = toml::to_string_pretty(self)
            .map_err(|e| FatalError::Config(format!("failed to serialize agent registry: {e}")))?;

        let path = base_dir.join("registry.toml");
        trace!(path = %path.display(), agents = self.agents.len(), "saving agent registry");
        std::fs::write(&path, contents).map_err(|e| {
            FatalError::Config(format!(
                "failed to write agent registry at {}: {e}",
                path.display()
            ))
        })
    }

    /// List all registered agents.
    #[must_use]
    pub fn list(&self) -> &[AgentEntry] {
        &self.agents
    }

    /// Look up an agent by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&AgentEntry> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// Add a new agent entry.
    pub fn add(&mut self, entry: AgentEntry) {
        self.agents.push(entry);
    }

    /// Remove an agent by name.
    pub fn remove(&mut self, name: &str) {
        self.agents.retain(|a| a.name != name);
    }

    /// Find the next available port starting from 7701.
    ///
    /// Scans existing entries and returns the lowest unused port.
    #[must_use]
    pub fn next_available_port(&self) -> u16 {
        let mut port = AGENT_PORT_START;
        while self.agents.iter().any(|a| a.port == port) {
            port = port.saturating_add(1);
        }
        let port_offset = port.saturating_sub(AGENT_PORT_START);
        if port_offset > MAX_EXPECTED_AGENTS {
            warn!(
                agents = self.agents.len(),
                port, port_offset, "port scan advanced far from starting port"
            );
        }
        port
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes directly for clarity"
)]
mod tests {
    use super::*;

    #[test]
    fn load_empty_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let reg = AgentRegistry::load(dir.path()).unwrap();
        assert!(reg.list().is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = AgentRegistry::default();
        reg.add(AgentEntry {
            name: "researcher".to_string(),
            port: 7701,
        });
        reg.add(AgentEntry {
            name: "coder".to_string(),
            port: 7702,
        });
        reg.save(dir.path()).unwrap();

        let loaded = AgentRegistry::load(dir.path()).unwrap();
        assert_eq!(loaded.list().len(), 2);
        assert_eq!(loaded.list()[0].name, "researcher");
        assert_eq!(loaded.list()[0].port, 7701);
        assert_eq!(loaded.list()[1].name, "coder");
        assert_eq!(loaded.list()[1].port, 7702);
    }

    #[test]
    fn get_finds_agent() {
        let mut reg = AgentRegistry::default();
        reg.add(AgentEntry {
            name: "researcher".to_string(),
            port: 7701,
        });
        assert!(reg.get("researcher").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn remove_removes_agent() {
        let mut reg = AgentRegistry::default();
        reg.add(AgentEntry {
            name: "researcher".to_string(),
            port: 7701,
        });
        reg.remove("researcher");
        assert!(reg.list().is_empty());
        reg.remove("nonexistent");
        assert!(reg.list().is_empty());
    }

    #[test]
    fn next_available_port_starts_at_7701() {
        let reg = AgentRegistry::default();
        assert_eq!(reg.next_available_port(), 7701);
    }

    #[test]
    fn next_available_port_skips_used() {
        let mut reg = AgentRegistry::default();
        reg.add(AgentEntry {
            name: "a".to_string(),
            port: 7701,
        });
        reg.add(AgentEntry {
            name: "b".to_string(),
            port: 7702,
        });
        assert_eq!(reg.next_available_port(), 7703);
    }

    #[test]
    fn next_available_port_fills_gaps() {
        let mut reg = AgentRegistry::default();
        reg.add(AgentEntry {
            name: "a".to_string(),
            port: 7701,
        });
        reg.add(AgentEntry {
            name: "b".to_string(),
            port: 7703,
        });
        assert_eq!(reg.next_available_port(), 7702);
    }

    #[test]
    fn toml_format_is_correct() {
        let mut reg = AgentRegistry::default();
        reg.add(AgentEntry {
            name: "researcher".to_string(),
            port: 7701,
        });
        let toml_str = toml::to_string_pretty(&reg).unwrap();
        assert!(
            toml_str.contains("[[agents]]"),
            "should use TOML array of tables: {toml_str}"
        );
        assert!(toml_str.contains("name = \"researcher\""));
        assert!(toml_str.contains("port = 7701"));
    }
}
