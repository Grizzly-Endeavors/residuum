//! MCP server registry and reconciliation.
//!
//! Tracks which MCP servers are running, manages live `McpClient` handles,
//! and exposes discovered tools to the agent's tool loop.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use crate::models::ToolDefinition;
use crate::projects::types::McpServerEntry;
use crate::tools::{ToolError, ToolResult};

use super::client::McpClient;

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

/// Public snapshot of a single MCP server's state (for external inspection).
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
    /// Cached tool definitions from this server.
    pub tools: Vec<ToolDefinition>,
}

/// Internal tracked server entry (holds the live client handle).
struct TrackedServer {
    name: String,
    command: String,
    args: Vec<String>,
    status: McpStatus,
    client: Option<McpClient>,
    tools: Vec<ToolDefinition>,
}

impl TrackedServer {
    /// Produce a public snapshot (without the client handle).
    fn snapshot(&self) -> McpServerState {
        McpServerState {
            name: self.name.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            status: self.status.clone(),
            tools: self.tools.clone(),
        }
    }
}

/// Result of a reconciliation diff (before connections are made).
#[derive(Debug, Default)]
pub struct McpReconcileResult {
    /// Servers that need to be started (in `desired` but not running).
    pub to_start: Vec<McpServerEntry>,
    /// Names of servers that need to be stopped (running but not in `desired`).
    pub to_stop: Vec<String>,
}

/// Report from `reconcile_and_connect` — how many servers started, stopped, or failed.
#[derive(Debug, Default)]
pub struct McpReconcileReport {
    /// Number of servers that connected successfully.
    pub started: usize,
    /// Number of servers that were stopped.
    pub stopped: usize,
    /// Servers that failed to start, with their errors.
    pub failures: Vec<(String, String)>,
}

/// Per-project activation state for MCP reference counting.
struct ProjectMcpState {
    /// Number of currently active agents that activated this project.
    active_count: usize,
    /// Server entries remembered for decrement-to-zero cleanup.
    servers: Vec<McpServerEntry>,
}

/// Registry tracking MCP server lifecycle state and live client handles.
pub struct McpRegistry {
    servers: Vec<TrackedServer>,
    /// Per-project activation reference counts.
    ///
    /// Keyed by lowercase project name. When multiple sub-agents activate the
    /// same project simultaneously, servers are shared and only disconnected
    /// when the last agent deactivates.
    project_refs: HashMap<String, ProjectMcpState>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
            project_refs: HashMap::new(),
        }
    }

    /// Create a new shared registry.
    #[must_use]
    pub fn new_shared() -> SharedMcpRegistry {
        Arc::new(RwLock::new(Self::new()))
    }

    /// Reconcile desired servers against current state (pure diff, no connections).
    ///
    /// Returns lists of servers to start and stop. The caller is responsible
    /// for acting on the result.
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
                    // Remove old entry if exists, add as pending
                    self.servers.retain(|s| s.name != entry.name);
                    self.servers.push(TrackedServer {
                        name: entry.name.clone(),
                        command: entry.command.clone(),
                        args: entry.args.clone(),
                        status: McpStatus::Pending,
                        client: None,
                        tools: Vec::new(),
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

        result.to_stop = to_stop;
        result
    }

    fn mark_failed_if_tracked(&mut self, name: &str, reason: &str) {
        if let Some(server) = self.servers.iter_mut().find(|s| s.name == name) {
            server.status = McpStatus::Failed(reason.to_string());
        }
    }

    /// Connect to an MCP server, list its tools, and mark it running.
    ///
    /// On failure, marks the server as failed.
    ///
    /// # Errors
    /// Returns the connection error (server is already marked failed internally).
    pub async fn connect(&mut self, entry: &McpServerEntry) -> Result<(), anyhow::Error> {
        tracing::debug!(server = %entry.name, "attempting mcp server connection");
        let client = match McpClient::connect(entry).await {
            Ok(c) => c,
            Err(e) => {
                self.mark_failed_if_tracked(&entry.name, &e.to_string());
                return Err(e);
            }
        };
        let tools = match client.list_tools().await {
            Ok(t) => t,
            Err(e) => {
                self.mark_failed_if_tracked(&entry.name, &e.to_string());
                return Err(e);
            }
        };

        if let Some(server) = self.servers.iter_mut().find(|s| s.name == entry.name) {
            server.status = McpStatus::Running;
            server.client = Some(client);
            server.tools = tools;
            tracing::info!(
                server = %entry.name,
                tool_count = server.tools.len(),
                "mcp server connected"
            );
        } else {
            tracing::warn!(
                server = %entry.name,
                "mcp server connected but was removed from tracking before state could be updated — client discarded"
            );
        }

        Ok(())
    }

    /// Disconnect a specific server by name.
    pub async fn disconnect(&mut self, name: &str) {
        if let Some(idx) = self.servers.iter().position(|s| s.name == name) {
            let server = self.servers.remove(idx);
            if let Some(client) = server.client {
                client.shutdown().await;
            }
            tracing::info!(server = %name, "mcp server disconnected");
        }
    }

    /// Disconnect all tracked servers.
    ///
    /// Returns names of servers that were disconnected.
    pub async fn disconnect_all(&mut self) -> Vec<String> {
        let servers: Vec<TrackedServer> = self.servers.drain(..).collect();
        let mut names = Vec::with_capacity(servers.len());

        for TrackedServer { name, client, .. } in servers {
            if let Some(c) = client {
                c.shutdown().await;
            }
            tracing::info!(server = %name, "mcp server disconnected");
            names.push(name);
        }

        names
    }

    /// Connect additional servers without reconciling existing state.
    ///
    /// Unlike `reconcile_and_connect`, this is purely additive — it never stops
    /// or removes servers that are already tracked. Servers that are already
    /// Running or Pending are silently skipped.
    pub async fn connect_servers(&mut self, entries: &[McpServerEntry]) -> McpReconcileReport {
        let mut report = McpReconcileReport::default();

        for entry in entries {
            let existing = self.servers.iter().find(|s| s.name == entry.name);
            if let Some(tracked) = existing {
                if tracked.status == McpStatus::Running || tracked.status == McpStatus::Pending {
                    continue;
                }
                // Remove failed entry so we can re-add as Pending
                self.servers.retain(|s| s.name != entry.name);
            }

            self.servers.push(TrackedServer {
                name: entry.name.clone(),
                command: entry.command.clone(),
                args: entry.args.clone(),
                status: McpStatus::Pending,
                client: None,
                tools: Vec::new(),
            });

            if let Err(e) = self.connect(entry).await {
                let reason = e.to_string();
                tracing::warn!(server = %entry.name, error = %reason, "mcp server failed to connect");
                report.failures.push((entry.name.clone(), reason));
            } else {
                report.started += 1;
            }
        }

        report
    }

    /// Reconcile and connect/disconnect in one step.
    ///
    /// Runs the state diff, then connects new servers and disconnects removed ones.
    pub async fn reconcile_and_connect(
        &mut self,
        desired: &[McpServerEntry],
    ) -> McpReconcileReport {
        let diff = self.reconcile(desired);
        let mut report = McpReconcileReport {
            started: 0,
            stopped: 0,
            failures: Vec::new(),
        };

        for entry in &diff.to_start {
            if let Err(e) = self.connect(entry).await {
                let reason = e.to_string();
                tracing::warn!(server = %entry.name, error = %reason, "mcp server failed to connect");
                report.failures.push((entry.name.clone(), reason));
            } else {
                report.started += 1;
            }
        }

        for name in &diff.to_stop {
            self.disconnect(name).await;
            report.stopped += 1;
        }

        report
    }

    /// Activate a project's MCP servers with reference counting.
    ///
    /// On first activation (count 0→1): reconciles and connects the project's
    /// servers. On subsequent activations: increments the reference count and
    /// returns an empty report (servers already running are reused).
    ///
    /// Multiple agents activating the same project share a single set of
    /// running servers. Servers are only stopped when the last agent deactivates.
    pub async fn activate_project(
        &mut self,
        project_name: &str,
        servers: &[McpServerEntry],
    ) -> McpReconcileReport {
        let key = project_name.to_lowercase();
        if let Some(state) = self.project_refs.get_mut(&key) {
            state.active_count += 1;
            tracing::debug!(
                project = %project_name,
                count = state.active_count,
                "project mcp ref count incremented (servers already running)"
            );
            return McpReconcileReport::default();
        }

        // First activation: start the servers and track them
        let report = self.reconcile_and_connect(servers).await;
        self.project_refs.insert(
            key,
            ProjectMcpState {
                active_count: 1,
                servers: servers.to_vec(),
            },
        );
        if !report.failures.is_empty() {
            tracing::warn!(
                project = %project_name,
                failures = report.failures.len(),
                "project mcp activation had connection failures"
            );
        }
        tracing::debug!(
            project = %project_name,
            started = report.started,
            "project mcp servers activated (first ref)"
        );
        report
    }

    /// Deactivate a project's MCP servers, decrementing the reference count.
    ///
    /// When the count reaches zero, the project's servers are disconnected.
    /// Returns the names of servers that were stopped (empty if count > 0).
    pub async fn deactivate_project(&mut self, project_name: &str) -> Vec<String> {
        let key = project_name.to_lowercase();
        let Some(state) = self.project_refs.get_mut(&key) else {
            return Vec::new();
        };

        if state.active_count == 0 {
            tracing::warn!(project = %project_name, "deactivate_project called with active_count already zero — possible double-deactivation");
            return Vec::new();
        }
        state.active_count -= 1;

        if state.active_count == 0 {
            let server_names: Vec<String> = state.servers.iter().map(|s| s.name.clone()).collect();
            self.project_refs.remove(&key);

            for name in &server_names {
                self.disconnect(name).await;
            }
            tracing::debug!(
                project = %project_name,
                count = server_names.len(),
                "project mcp servers stopped (last ref)"
            );
            server_names
        } else {
            tracing::debug!(
                project = %project_name,
                count = state.active_count,
                "project mcp ref count decremented (servers still in use)"
            );
            Vec::new()
        }
    }

    /// Force-deactivate a project's MCP servers regardless of reference count.
    ///
    /// Sets the count to zero and disconnects immediately. Used for crash
    /// recovery when a sub-agent exits without calling `deactivate_project`.
    /// Returns the names of servers that were stopped.
    pub async fn force_deactivate_project(&mut self, project_name: &str) -> Vec<String> {
        let key = project_name.to_lowercase();
        let Some(state) = self.project_refs.remove(&key) else {
            return Vec::new();
        };

        let stopped: Vec<String> = state.servers.iter().map(|s| s.name.clone()).collect();
        for name in &stopped {
            self.disconnect(name).await;
        }
        tracing::warn!(
            project = %project_name,
            count = stopped.len(),
            "project mcp servers force-stopped (ref count bypassed — crash recovery)"
        );
        stopped
    }

    /// Get tool definitions from all running servers (flat union).
    #[must_use]
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.servers
            .iter()
            .filter(|s| s.status == McpStatus::Running)
            .flat_map(|s| s.tools.iter().cloned())
            .collect()
    }

    /// Call a tool by name, routing to the server that owns it.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no running server has the tool.
    /// Returns `ToolError::Execution` if the RPC call fails.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<ToolResult, ToolError> {
        let server = self
            .servers
            .iter()
            .filter(|s| s.status == McpStatus::Running)
            .find(|s| s.tools.iter().any(|t| t.name == name));

        let server = server.ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        let client = server.client.as_ref().ok_or_else(|| {
            tracing::error!(
                server = %server.name,
                "running server has no client handle — internal state corruption"
            );
            ToolError::Execution(format!(
                "mcp server '{}' is marked running but has no client",
                server.name
            ))
        })?;

        tracing::debug!(tool = %name, server = %server.name, "routing tool call to server");
        client.call_tool(name, args).await
    }

    /// Mark a server as running (used in tests without a live client).
    #[doc(hidden)]
    pub fn mark_running(&mut self, name: &str) {
        if let Some(server) = self.servers.iter_mut().find(|s| s.name == name) {
            server.status = McpStatus::Running;
        }
    }

    /// Mark a server as stopped and remove it from tracking.
    #[cfg(test)]
    pub fn mark_stopped(&mut self, name: &str) {
        self.servers.retain(|s| s.name != name);
    }

    /// Mark a server as failed with a reason.
    #[cfg(test)]
    pub fn mark_failed(&mut self, name: &str, reason: &str) {
        if let Some(server) = self.servers.iter_mut().find(|s| s.name == name) {
            server.status = McpStatus::Failed(reason.to_string());
        }
    }

    /// Stop all tracked servers without async shutdown.
    ///
    /// Returns names of servers that were running or pending.
    /// Clients are dropped (child processes killed via `ChildWithCleanup::drop`).
    #[cfg(test)]
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
    pub fn servers(&self) -> Vec<McpServerState> {
        self.servers.iter().map(TrackedServer::snapshot).collect()
    }
}

impl std::fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRegistry")
            .field("server_count", &self.servers.len())
            .field("project_ref_count", &self.project_refs.len())
            .finish_non_exhaustive()
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
            env: std::collections::HashMap::new(),
            transport: crate::projects::types::McpTransport::default(),
            headers: std::collections::HashMap::new(),
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
        assert_eq!(
            registry.servers().len(),
            2,
            "reconcile does not remove servers; caller handles graceful shutdown"
        );
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
        let servers = registry.servers();
        let server = servers.first().unwrap();
        assert_eq!(
            server.status,
            McpStatus::Failed("connection refused".to_string()),
            "status should be Failed"
        );
    }

    #[test]
    fn tool_definitions_empty_when_no_running() {
        let registry = McpRegistry::new();
        assert!(
            registry.tool_definitions().is_empty(),
            "should be empty with no servers"
        );
    }

    #[tokio::test]
    async fn call_tool_not_found() {
        let registry = McpRegistry::new();
        let result = registry
            .call_tool("nonexistent", serde_json::json!({}))
            .await;
        assert!(result.is_err(), "should error for unknown tool");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ToolError::NotFound(_)),
            "should be NotFound error"
        );
    }

    // ── activate_project / deactivate_project / force_deactivate ─────────────

    #[tokio::test]
    async fn activate_project_first_time_records_state() {
        // Verify that activation tracks the project even when server connect fails.
        // On first activation: project_refs entry is created.
        // On second activation: count increments and empty report returned.
        let mut registry = McpRegistry::new();

        let report1 = registry
            .activate_project("testproject", &[entry("fs", "/nonexistent")])
            .await;
        assert_eq!(
            report1.failures.len(),
            1,
            "server connect fails (no binary)"
        );

        // Second activation: no new connections started
        let report2 = registry
            .activate_project("testproject", &[entry("fs", "/nonexistent")])
            .await;
        assert_eq!(report2.started, 0, "second activation returns empty report");
        assert_eq!(report2.failures.len(), 0, "no re-connect attempted");

        // Project entry should still exist (first deactivation won't stop servers)
        let first_deact = registry.deactivate_project("testproject").await;
        assert!(first_deact.is_empty(), "count 2→1, no servers stopped");

        // Second deactivation clears the entry
        let second_deact = registry.deactivate_project("testproject").await;
        assert_eq!(second_deact, vec!["fs"], "count 1→0, server name returned");
        assert!(
            !registry.project_refs.contains_key("testproject"),
            "project entry removed at count 0"
        );
    }

    #[tokio::test]
    async fn deactivate_project_decrements_count() {
        let mut registry = McpRegistry::new();
        // Simulate two activations by inserting directly
        registry.project_refs.insert(
            "myproject".to_string(),
            ProjectMcpState {
                active_count: 2,
                servers: vec![entry("svc", "mcp-svc")],
            },
        );
        // Track the server as pending (no real client)
        registry.reconcile(&[entry("svc", "mcp-svc")]);

        // First deactivation: count 2 → 1, no servers stopped
        let first_stopped = registry.deactivate_project("myproject").await;
        assert!(
            first_stopped.is_empty(),
            "count > 0, no servers should be stopped"
        );
        // Project entry should still exist (entry still has count = 1)
        assert!(
            registry.project_refs.contains_key("myproject"),
            "project still tracked at count 1"
        );

        // Second deactivation: count 1 → 0, servers stopped
        let second_stopped = registry.deactivate_project("myproject").await;
        assert_eq!(
            second_stopped,
            vec!["svc"],
            "server should be stopped at count 0"
        );
        assert!(
            !registry.project_refs.contains_key("myproject"),
            "project entry should be removed"
        );
    }

    #[tokio::test]
    async fn deactivate_project_unknown_is_noop() {
        let mut registry = McpRegistry::new();
        let stopped = registry.deactivate_project("unknown").await;
        assert!(
            stopped.is_empty(),
            "deactivating unknown project is a no-op"
        );
    }

    #[tokio::test]
    async fn force_deactivate_project_ignores_count() {
        let mut registry = McpRegistry::new();
        // Simulate 3 active refs
        registry.project_refs.insert(
            "bigproject".to_string(),
            ProjectMcpState {
                active_count: 3,
                servers: vec![entry("alpha", "mcp-alpha"), entry("beta", "mcp-beta")],
            },
        );
        registry.reconcile(&[entry("alpha", "mcp-alpha"), entry("beta", "mcp-beta")]);

        let stopped = registry.force_deactivate_project("bigproject").await;
        assert_eq!(
            stopped.len(),
            2,
            "both servers should be reported as stopped"
        );
        assert!(
            !registry.project_refs.contains_key("bigproject"),
            "project entry should be removed"
        );
    }

    #[tokio::test]
    async fn force_deactivate_project_unknown_is_noop() {
        let mut registry = McpRegistry::new();
        let stopped = registry.force_deactivate_project("unknown").await;
        assert!(
            stopped.is_empty(),
            "force deactivating unknown project is a no-op"
        );
    }

    // ── connect_servers (additive, no reconciliation) ──────────────────────

    #[tokio::test]
    async fn connect_servers_does_not_remove_existing() {
        let mut registry = McpRegistry::new();

        // Pre-populate with two running servers
        registry.reconcile(&[entry("fs", "mcp-fs"), entry("git", "mcp-git")]);
        registry.mark_running("fs");
        registry.mark_running("git");

        // connect_servers with a new entry (will fail to connect, but that's fine)
        let report = registry
            .connect_servers(&[entry("new-server", "/nonexistent")])
            .await;

        // The new server should fail, but existing servers must still be tracked
        assert_eq!(report.failures.len(), 1, "new server should fail");
        assert_eq!(report.stopped, 0, "nothing should be stopped");

        let servers = registry.servers();
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"fs"), "fs should still be tracked");
        assert!(names.contains(&"git"), "git should still be tracked");
        assert!(
            names.contains(&"new-server"),
            "new-server should be tracked (failed)"
        );
    }

    #[tokio::test]
    async fn connect_servers_skips_already_running() {
        let mut registry = McpRegistry::new();

        // Pre-populate with a running server
        registry.reconcile(&[entry("fs", "mcp-fs")]);
        registry.mark_running("fs");

        // Attempt to connect the same server again
        let report = registry.connect_servers(&[entry("fs", "mcp-fs")]).await;

        assert_eq!(report.started, 0, "should not re-connect running server");
        assert!(report.failures.is_empty(), "no failures expected");
        assert_eq!(report.stopped, 0, "nothing stopped");

        // Server should still be running (unchanged)
        let servers = registry.servers();
        assert_eq!(servers.len(), 1, "still one server");
        assert_eq!(servers.first().unwrap().status, McpStatus::Running);
    }

    #[tokio::test]
    async fn reconcile_and_connect_nonexistent_fails_gracefully() {
        let mut registry = McpRegistry::new();
        let desired = vec![entry("bad", "/nonexistent/mcp-server")];

        let report = registry.reconcile_and_connect(&desired).await;
        assert_eq!(report.started, 0, "nothing should start");
        assert_eq!(report.failures.len(), 1, "should have one failure");
        assert_eq!(
            report.failures.first().unwrap().0,
            "bad",
            "failed server name"
        );

        // Server should be marked failed
        let servers = registry.servers();
        let server = servers.first().unwrap();
        assert!(
            matches!(server.status, McpStatus::Failed(_)),
            "server should be marked failed"
        );
    }
}
