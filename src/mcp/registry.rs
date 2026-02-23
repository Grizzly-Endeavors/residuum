//! MCP server registry and reconciliation.
//!
//! Tracks which MCP servers are running and provides a reconciliation
//! interface for project activation/deactivation. The actual launch pipeline
//! (process spawning, protocol negotiation, tool registration) is handled
//! separately — this module only manages desired-vs-running state.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::projects::types::McpServerEntry;

/// Shared MCP registry, accessible from project tools and the gateway.
pub type SharedMcpRegistry = Arc<RwLock<McpRegistry>>;

/// Lifecycle status of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    /// Server start has been requested but not yet confirmed.
    Pending,
    /// Server is running and ready.
    Running,
    /// Server failed to start or crashed.
    Failed(String),
}

/// Tracked state of a single MCP server.
#[derive(Debug, Clone)]
pub struct McpServerState {
    /// Server name (matches `McpServerEntry::name`).
    pub name: String,
    /// Command used to start the server.
    pub command: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Current lifecycle status.
    pub status: McpStatus,
}

/// Result of a reconciliation pass.
#[derive(Debug, Default)]
pub struct McpReconcileResult {
    /// Servers that need to be started (in `desired` but not running).
    pub to_start: Vec<McpServerEntry>,
    /// Names of servers that need to be stopped (running but not in `desired`).
    pub to_stop: Vec<String>,
}

/// Registry tracking MCP server lifecycle state.
#[derive(Debug, Default)]
pub struct McpRegistry {
    servers: Vec<McpServerState>,
}

impl McpRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
        }
    }

    /// Create a new shared registry.
    #[must_use]
    pub fn new_shared() -> SharedMcpRegistry {
        Arc::new(RwLock::new(Self::new()))
    }

    /// Reconcile desired servers (from project frontmatter) against current state.
    ///
    /// Returns lists of servers to start and stop. Does not perform the actual
    /// start/stop — the caller feeds the result to the launch pipeline.
    pub fn reconcile(&mut self, desired: &[McpServerEntry]) -> McpReconcileResult {
        let mut result = McpReconcileResult::default();

        // Servers in desired but not currently tracked (or failed) → to_start
        for entry in desired {
            let existing = self.servers.iter().find(|s| s.name == entry.name);
            match existing {
                Some(s) if s.status == McpStatus::Running || s.status == McpStatus::Pending => {
                    // Already running or starting — no-op
                }
                _ => {
                    result.to_start.push(entry.clone());
                    // Add as pending
                    self.servers.retain(|s| s.name != entry.name);
                    self.servers.push(McpServerState {
                        name: entry.name.clone(),
                        command: entry.command.clone(),
                        args: entry.args.clone(),
                        status: McpStatus::Pending,
                    });
                }
            }
        }

        // Servers currently tracked but not in desired → to_stop
        let desired_names: std::collections::HashSet<&str> =
            desired.iter().map(|e| e.name.as_str()).collect();
        let to_stop: Vec<String> = self
            .servers
            .iter()
            .filter(|s| !desired_names.contains(s.name.as_str()))
            .filter(|s| s.status == McpStatus::Running || s.status == McpStatus::Pending)
            .map(|s| s.name.clone())
            .collect();

        for name in &to_stop {
            self.servers.retain(|s| s.name != *name);
        }

        result.to_stop = to_stop;
        result
    }

    /// Mark a server as running.
    pub fn mark_running(&mut self, name: &str) {
        if let Some(server) = self.servers.iter_mut().find(|s| s.name == name) {
            server.status = McpStatus::Running;
        }
    }

    /// Mark a server as stopped and remove it from tracking.
    pub fn mark_stopped(&mut self, name: &str) {
        self.servers.retain(|s| s.name != name);
    }

    /// Mark a server as failed with a reason.
    pub fn mark_failed(&mut self, name: &str, reason: &str) {
        if let Some(server) = self.servers.iter_mut().find(|s| s.name == name) {
            server.status = McpStatus::Failed(reason.to_string());
        }
    }

    /// Stop all tracked servers. Returns names of servers that were running or pending.
    ///
    /// Called on project deactivation.
    pub fn stop_all(&mut self) -> Vec<String> {
        let names: Vec<String> = self
            .servers
            .iter()
            .filter(|s| s.status == McpStatus::Running || s.status == McpStatus::Pending)
            .map(|s| s.name.clone())
            .collect();

        self.servers.clear();
        names
    }

    /// Get a snapshot of all tracked servers.
    #[must_use]
    pub fn servers(&self) -> &[McpServerState] {
        &self.servers
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    fn entry(name: &str, command: &str) -> McpServerEntry {
        McpServerEntry {
            name: name.to_string(),
            command: command.to_string(),
            args: vec![],
        }
    }

    #[test]
    fn reconcile_empty_desired_no_ops() {
        let mut registry = McpRegistry::new();
        let result = registry.reconcile(&[]);
        assert!(result.to_start.is_empty(), "nothing to start");
        assert!(result.to_stop.is_empty(), "nothing to stop");
    }

    #[test]
    fn reconcile_starts_new_servers() {
        let mut registry = McpRegistry::new();
        let desired = vec![entry("fs", "mcp-fs"), entry("git", "mcp-git")];

        let result = registry.reconcile(&desired);
        assert_eq!(result.to_start.len(), 2, "should start both servers");
        assert!(result.to_stop.is_empty(), "nothing to stop");
        assert_eq!(
            registry.servers().len(),
            2,
            "both should be tracked as pending"
        );
    }

    #[test]
    fn reconcile_stops_removed_servers() {
        let mut registry = McpRegistry::new();

        // Start with two servers running
        let initial = vec![entry("fs", "mcp-fs"), entry("git", "mcp-git")];
        registry.reconcile(&initial);
        registry.mark_running("fs");
        registry.mark_running("git");

        // Desired now only has fs
        let result = registry.reconcile(&[entry("fs", "mcp-fs")]);
        assert!(result.to_start.is_empty(), "fs already running");
        assert_eq!(result.to_stop, vec!["git"], "git should be stopped");
        assert_eq!(registry.servers().len(), 1, "only fs should remain tracked");
    }

    #[test]
    fn reconcile_skips_already_running() {
        let mut registry = McpRegistry::new();
        registry.reconcile(&[entry("fs", "mcp-fs")]);
        registry.mark_running("fs");

        let result = registry.reconcile(&[entry("fs", "mcp-fs")]);
        assert!(
            result.to_start.is_empty(),
            "should not restart running server"
        );
        assert!(result.to_stop.is_empty(), "nothing to stop");
    }

    #[test]
    fn reconcile_restarts_failed_servers() {
        let mut registry = McpRegistry::new();
        registry.reconcile(&[entry("fs", "mcp-fs")]);
        registry.mark_failed("fs", "crashed");

        let result = registry.reconcile(&[entry("fs", "mcp-fs")]);
        assert_eq!(
            result.to_start.len(),
            1,
            "failed server should be restarted"
        );
        assert_eq!(
            result.to_start.first().unwrap().name,
            "fs",
            "should restart fs"
        );
    }

    #[test]
    fn stop_all_returns_running_names() {
        let mut registry = McpRegistry::new();
        registry.reconcile(&[entry("fs", "mcp-fs"), entry("git", "mcp-git")]);
        registry.mark_running("fs");
        // git stays pending

        let stopped = registry.stop_all();
        assert_eq!(stopped.len(), 2, "should return both");
        assert!(stopped.contains(&"fs".to_string()), "should include fs");
        assert!(stopped.contains(&"git".to_string()), "should include git");
        assert!(
            registry.servers().is_empty(),
            "all servers should be cleared"
        );
    }

    #[test]
    fn mark_stopped_removes_server() {
        let mut registry = McpRegistry::new();
        registry.reconcile(&[entry("fs", "mcp-fs")]);
        registry.mark_running("fs");

        registry.mark_stopped("fs");
        assert!(registry.servers().is_empty(), "fs should be removed");
    }

    #[test]
    fn mark_failed_updates_status() {
        let mut registry = McpRegistry::new();
        registry.reconcile(&[entry("fs", "mcp-fs")]);

        registry.mark_failed("fs", "connection refused");
        let server = registry.servers().first().unwrap();
        assert_eq!(
            server.status,
            McpStatus::Failed("connection refused".to_string()),
            "status should be Failed"
        );
    }
}
