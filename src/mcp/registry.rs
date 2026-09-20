//! MCP server registry and reconciliation.
//!
//! Tracks which MCP servers are running, manages live `McpClient` handles,
//! and exposes discovered tools to the agent's tool loop.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use crate::models::ToolDefinition;
use crate::tools::{SharedToolsPath, ToolError, ToolResult};

use super::client::McpClient;
use super::types::McpServerEntry;

/// Shared MCP registry, accessible from the gateway.
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

/// Registry tracking MCP server lifecycle state and live client handles.
pub struct McpRegistry {
    servers: Vec<TrackedServer>,
    /// Effective `PATH` handle prepended to stdio servers' `PATH` at spawn.
    ///
    /// `None` in bare registries (tests); the gateway injects the configured
    /// handle so tool dirs become resolvable. An entry's own `PATH` in its
    /// `env` still wins (applied after the injected value at spawn time).
    tools_path: Option<SharedToolsPath>,
    /// Names of built-in agent tools that reserve the shared tool namespace.
    ///
    /// A built-in always wins a name collision (it is dispatched first in the
    /// turn loop), so an MCP tool that reuses one of these names is shadowed:
    /// excluded from [`tool_definitions`](Self::tool_definitions) and never
    /// reachable. Set once by the gateway after the tool registry is built via
    /// [`set_reserved_tool_names`](Self::set_reserved_tool_names); empty in
    /// bare registries (tests). See `src/mcp/CLAUDE.md` for the full policy.
    reserved_tool_names: HashSet<String>,
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
            tools_path: None,
            reserved_tool_names: HashSet::new(),
        }
    }

    /// Create a new shared registry.
    #[must_use]
    pub fn new_shared() -> SharedMcpRegistry {
        Arc::new(RwLock::new(Self::new()))
    }

    /// Create a new shared registry that prepends the configured tool
    /// directories to the `PATH` of spawned stdio servers.
    #[must_use]
    pub fn new_shared_with_tools_path(tools_path: SharedToolsPath) -> SharedMcpRegistry {
        Arc::new(RwLock::new(Self {
            servers: Vec::new(),
            tools_path: Some(tools_path),
            reserved_tool_names: HashSet::new(),
        }))
    }

    /// Reserve the built-in tool namespace so colliding MCP tools are shadowed
    /// visibly rather than silently.
    ///
    /// Call once after the built-in tool registry is built. Servers that
    /// connected earlier (e.g. workspace servers started before the tool
    /// registry existed) are re-scanned here so their collisions are still
    /// reported; servers that connect later are checked at connect time.
    pub fn set_reserved_tool_names(&mut self, names: impl IntoIterator<Item = String>) {
        self.reserved_tool_names = names.into_iter().collect();
        for server in self
            .servers
            .iter()
            .filter(|s| s.status == McpStatus::Running)
        {
            for tool in &server.tools {
                if self.reserved_tool_names.contains(&tool.name) {
                    tracing::warn!(
                        tool = %tool.name,
                        mcp.server = %server.name,
                        "mcp tool name collides with a built-in tool; the built-in wins and the mcp tool is shadowed (unreachable)"
                    );
                }
            }
        }
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
    #[tracing::instrument(skip_all, fields(mcp.server = %entry.name))]
    pub async fn connect(&mut self, entry: &McpServerEntry) -> Result<(), anyhow::Error> {
        tracing::debug!("attempting mcp server connection");
        // Snapshot the effective PATH (read live so config reloads apply).
        let tools_path = match &self.tools_path {
            Some(handle) => handle.read().await.clone(),
            None => None,
        };
        let client = match McpClient::connect(entry, tools_path.as_deref()).await {
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

        // Detect name collisions before exposing this server's tools. The
        // check runs against reserved built-in names and already-running
        // servers, so it must happen before the new server is marked running.
        self.warn_shadowed_tools(&entry.name, &tools);

        if let Some(server) = self.servers.iter_mut().find(|s| s.name == entry.name) {
            server.status = McpStatus::Running;
            server.client = Some(client);
            server.tools = tools;
            tracing::info!(tool_count = server.tools.len(), "mcp server connected");
        } else {
            tracing::warn!(
                mcp.server = %entry.name,
                "mcp server connected but was removed from tracking before state could be updated — client discarded"
            );
        }

        Ok(())
    }

    /// Disconnect a specific server by name.
    #[tracing::instrument(skip_all, fields(mcp.server = %name))]
    pub async fn disconnect(&mut self, name: &str) {
        if let Some(idx) = self.servers.iter().position(|s| s.name == name) {
            let server = self.servers.remove(idx);
            if let Some(client) = server.client {
                client.shutdown().await;
            }
            tracing::info!("mcp server disconnected");
        }
    }

    /// Disconnect all tracked servers.
    ///
    /// Returns names of servers that were disconnected.
    #[tracing::instrument(skip_all)]
    pub async fn disconnect_all(&mut self) -> Vec<String> {
        let mut names = Vec::with_capacity(self.servers.len());

        for TrackedServer { name, client, .. } in self.servers.drain(..) {
            if let Some(c) = client {
                c.shutdown().await;
            }
            names.push(name);
        }

        tracing::info!(count = names.len(), "mcp servers disconnected");
        names
    }

    async fn attempt_connect(&mut self, entry: &McpServerEntry, report: &mut McpReconcileReport) {
        if let Err(e) = self.connect(entry).await {
            let reason = e.to_string();
            tracing::warn!(server = %entry.name, error = %reason, "mcp server failed to connect");
            report.failures.push((entry.name.clone(), reason));
        } else {
            report.started += 1;
        }
    }

    /// Connect additional servers without reconciling existing state.
    ///
    /// Unlike `reconcile_and_connect`, this is purely additive — it never stops
    /// or removes servers that are already tracked. Servers that are already
    /// Running or Pending are silently skipped.
    #[tracing::instrument(skip_all, fields(server_count = entries.len()))]
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

            self.attempt_connect(entry, &mut report).await;
        }

        report
    }

    /// Reconcile and connect/disconnect in one step.
    ///
    /// Runs the state diff, then connects new servers and disconnects removed ones.
    #[tracing::instrument(skip_all)]
    pub async fn reconcile_and_connect(
        &mut self,
        desired: &[McpServerEntry],
    ) -> McpReconcileReport {
        let diff = self.reconcile(desired);
        let mut report = McpReconcileReport::default();

        for entry in &diff.to_start {
            self.attempt_connect(entry, &mut report).await;
        }

        for name in &diff.to_stop {
            self.disconnect(name).await;
            report.stopped += 1;
        }

        report
    }

    /// Warn about tool-name collisions for a server that is about to expose
    /// `tools`, without changing which tools win.
    ///
    /// Precedence is fixed so shadowing is deterministic: a built-in tool
    /// always wins (it is dispatched first in the turn loop), and among MCP
    /// servers the first-registered running server wins. This server's
    /// colliding tools are therefore shadowed — excluded from
    /// [`tool_definitions`](Self::tool_definitions) and never dispatched to.
    /// The winner and this server's non-colliding tools keep working. See
    /// `src/mcp/CLAUDE.md`.
    fn warn_shadowed_tools(&self, server_name: &str, tools: &[ToolDefinition]) {
        for tool in tools {
            if self.reserved_tool_names.contains(&tool.name) {
                tracing::warn!(
                    tool = %tool.name,
                    mcp.server = %server_name,
                    "mcp tool name collides with a built-in tool; the built-in wins and the mcp tool is shadowed (unreachable)"
                );
            } else if let Some(owner) = self.running_owner_of_tool(&tool.name) {
                tracing::warn!(
                    tool = %tool.name,
                    mcp.server = %server_name,
                    winner = %owner,
                    "mcp tool name collides with a tool from another mcp server; the first-registered server wins and this one is shadowed (unreachable)"
                );
            }
        }
    }

    /// Name of the first running server that already exposes `tool_name`, if any.
    fn running_owner_of_tool(&self, tool_name: &str) -> Option<&str> {
        self.servers
            .iter()
            .filter(|s| s.status == McpStatus::Running)
            .find(|s| s.tools.iter().any(|t| t.name == tool_name))
            .map(|s| s.name.as_str())
    }

    /// Get tool definitions from all running servers.
    ///
    /// The result is de-duplicated so the model is offered each name exactly
    /// once, matching what [`call_tool`](Self::call_tool) will dispatch to:
    /// names reserved by a built-in tool are excluded (the built-in wins), and
    /// when two MCP servers expose the same name only the first-registered
    /// running server's definition is kept. See `src/mcp/CLAUDE.md`.
    #[must_use]
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut defs: Vec<ToolDefinition> = Vec::new();
        for server in self
            .servers
            .iter()
            .filter(|s| s.status == McpStatus::Running)
        {
            for tool in &server.tools {
                // A built-in tool of this name always wins; skip the shadowed
                // MCP tool so the model is never offered an unreachable name.
                if self.reserved_tool_names.contains(&tool.name) {
                    continue;
                }
                // First running server to claim a name wins; later duplicates
                // are shadowed.
                if seen.insert(tool.name.as_str()) {
                    defs.push(tool.clone());
                }
            }
        }
        defs
    }

    /// Call a tool by name, routing to the server that owns it.
    ///
    /// When two running servers expose the same name the first-registered one
    /// wins, matching the de-duplication in
    /// [`tool_definitions`](Self::tool_definitions). Names reserved by a
    /// built-in tool never reach here: the turn loop dispatches built-ins
    /// first and only falls back to MCP when no built-in matches.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no running server has the tool.
    /// Returns `ToolError::Execution` if the RPC call fails.
    #[tracing::instrument(skip_all, fields(mcp.tool = %name))]
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<ToolResult, ToolError> {
        let server = self
            .servers
            .iter()
            .filter(|s| s.status == McpStatus::Running)
            .find(|s| s.tools.iter().any(|t| t.name == name));

        let server = server.ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        let client = server.client.as_ref().ok_or_else(|| {
            tracing::error!(
                mcp.server = %server.name,
                "running server has no client handle — internal state corruption"
            );
            ToolError::Execution(format!(
                "mcp server '{}' is marked running but has no client",
                server.name
            ))
        })?;

        tracing::debug!(mcp.server = %server.name, "routing tool call to server");
        client.call_tool(name, args).await
    }

    /// Mark a server as running (used in tests without a live client).
    // `pub` (not `#[cfg(test)]`) because integration tests in tests/ need this.
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
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, command: &str) -> McpServerEntry {
        McpServerEntry {
            name: name.to_string(),
            command: command.to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            transport: crate::mcp::types::McpTransport::default(),
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
        let names: Vec<&str> = result.to_start.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"fs") && names.contains(&"git"),
            "to_start should contain fs and git"
        );
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
    fn reconcile_skips_already_pending() {
        let mut registry = McpRegistry::new();
        // First reconcile adds server as Pending; do NOT call mark_running
        registry.reconcile(&[entry("fs", "mcp-fs")]);

        let result = registry.reconcile(&[entry("fs", "mcp-fs")]);
        assert!(
            result.to_start.is_empty(),
            "should not restart pending server"
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

    fn tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("desc for {name}"),
            parameters: serde_json::json!({}),
        }
    }

    /// Push a running server with the given tools directly (bypassing connect,
    /// which needs a live client).
    fn push_running_server(registry: &mut McpRegistry, name: &str, tools: &[&str]) {
        registry.servers.push(TrackedServer {
            name: name.to_string(),
            command: "cmd".to_string(),
            args: vec![],
            status: McpStatus::Running,
            client: None,
            tools: tools.iter().map(|t| tool_def(t)).collect(),
        });
    }

    #[test]
    fn tool_definitions_dedupes_mcp_collisions_first_server_wins() {
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "first", &["shared", "only_first"]);
        push_running_server(&mut registry, "second", &["shared", "only_second"]);

        let defs = registry.tool_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

        assert_eq!(
            names.iter().filter(|n| **n == "shared").count(),
            1,
            "colliding name should appear exactly once"
        );
        // The kept definition is the first server's (first-registered wins).
        let shared = defs.iter().find(|d| d.name == "shared").unwrap();
        assert_eq!(
            shared.description, "desc for shared",
            "definition is the winner's, not a merge"
        );
        assert!(names.contains(&"only_first"), "unique tools still exposed");
        assert!(names.contains(&"only_second"), "unique tools still exposed");
    }

    #[test]
    fn tool_definitions_excludes_names_reserved_by_builtins() {
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "srv", &["exec", "mcp_only"]);
        registry.set_reserved_tool_names(["exec".to_string(), "read_file".to_string()]);

        let defs = registry.tool_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

        assert!(
            !names.contains(&"exec"),
            "mcp tool colliding with a built-in must be shadowed (not offered)"
        );
        assert!(
            names.contains(&"mcp_only"),
            "non-colliding mcp tool stays available"
        );
    }

    #[tokio::test]
    async fn call_tool_routes_collision_to_first_registered_server() {
        // Both servers claim "shared"; first-registered wins the routing.
        // Neither has a live client, so the winner surfaces the "no client"
        // execution error — proving the call was routed to it, not NotFound.
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "first", &["shared"]);
        push_running_server(&mut registry, "second", &["shared"]);

        let err = registry
            .call_tool("shared", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ToolError::Execution(msg) if msg.contains("first")),
            "call should route to the first-registered server, got: {err:?}"
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
        assert_eq!(
            err,
            ToolError::NotFound("nonexistent".to_string()),
            "should be NotFound carrying the tool name"
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
