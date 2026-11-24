use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use residuum_config::SharedToolsPath;
use residuum_config::agent_keys::SharedAgentKeys;
use residuum_inference::ToolDefinition;
use residuum_inference::{ToolError, ToolResult};

use super::client::{McpClient, McpClientHandle};
use super::types::McpServerEntry;

pub type SharedMcpRegistry = Arc<RwLock<McpRegistry>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    Pending,
    Running,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct McpServerState {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub status: McpStatus,
    pub tools: Vec<ToolDefinition>,
}

struct TrackedServer {
    name: String,
    command: String,
    args: Vec<String>,
    status: McpStatus,
    client: Option<McpClient>,
    tools: Vec<ToolDefinition>,
}

impl TrackedServer {
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

fn in_name_order(mut tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

#[derive(Debug, Default)]
pub(crate) struct McpReconcileResult {
    pub(crate) to_start: Vec<McpServerEntry>,
    pub(crate) to_stop: Vec<String>,
}

#[derive(Debug, Default)]
pub struct McpReconcileReport {
    pub started: usize,
    pub stopped: usize,
    pub failures: Vec<(String, String)>,
}

pub struct McpRegistry {
    servers: BTreeMap<String, TrackedServer>,
    tools_path: Option<SharedToolsPath>,
    agent_keys: Option<SharedAgentKeys>,
    secrets_dir: Option<PathBuf>,
    reserved_tool_names: BTreeSet<String>,
    workspace_root: Option<PathBuf>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            servers: BTreeMap::new(),
            tools_path: None,
            agent_keys: None,
            secrets_dir: None,
            reserved_tool_names: BTreeSet::new(),
            workspace_root: None,
        }
    }

    #[must_use]
    pub fn new_shared() -> SharedMcpRegistry {
        Arc::new(RwLock::new(Self::new()))
    }

    #[must_use]
    pub fn new_shared_with_spawn_env(
        tools_path: SharedToolsPath,
        agent_keys: SharedAgentKeys,
        secrets_dir: PathBuf,
        workspace_root: PathBuf,
    ) -> SharedMcpRegistry {
        Arc::new(RwLock::new(Self {
            servers: BTreeMap::new(),
            tools_path: Some(tools_path),
            agent_keys: Some(agent_keys),
            secrets_dir: Some(secrets_dir),
            reserved_tool_names: BTreeSet::new(),
            workspace_root: Some(workspace_root),
        }))
    }

    async fn resolve_agent_keys<'e>(
        &self,
        entry: &'e McpServerEntry,
    ) -> Result<std::borrow::Cow<'e, McpServerEntry>, anyhow::Error> {
        let referenced = entry
            .env
            .values()
            .chain(entry.headers.values())
            .any(|v| residuum_config::agent_keys::has_references(v));
        if !referenced {
            return Ok(std::borrow::Cow::Borrowed(entry));
        }
        let Some(keys) = &self.agent_keys else {
            anyhow::bail!(
                "mcp server '{}' references agent keys, but no agent key store is available",
                entry.name
            );
        };
        let snapshot = keys.snapshot().await.map_err(|e| {
            anyhow::anyhow!("mcp server '{}' references agent keys: {e}", entry.name)
        })?;
        let expand = |map: &std::collections::BTreeMap<String, String>, kind: &str| {
            map.iter()
                .map(|(k, v)| {
                    residuum_config::agent_keys::expand_references(v, &snapshot.store)
                        .map(|expanded| (k.clone(), expanded))
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "mcp server '{}' {kind} '{k}' references an agent key: {e}",
                                entry.name
                            )
                        })
                })
                .collect::<Result<std::collections::BTreeMap<_, _>, _>>()
        };
        let mut resolved = entry.clone();
        resolved.env = expand(&entry.env, "env var")?;
        resolved.headers = expand(&entry.headers, "header")?;
        Ok(std::borrow::Cow::Owned(resolved))
    }

    async fn resolve_secrets<'e>(
        &self,
        entry: &'e McpServerEntry,
    ) -> Result<std::borrow::Cow<'e, McpServerEntry>, anyhow::Error> {
        let referenced = entry
            .env
            .values()
            .chain(entry.headers.values())
            .any(|v| v.starts_with("secret:"));
        if !referenced {
            return Ok(std::borrow::Cow::Borrowed(entry));
        }
        let Some(dir) = self.secrets_dir.clone() else {
            anyhow::bail!(
                "mcp server '{}' references a secret, but no secret store is available",
                entry.name
            );
        };
        let store = residuum_core::util::spawn_blocking_in_span(move || {
            residuum_config::secrets::SecretStore::load(&dir)
        })
        .await
        .map_err(|e| anyhow::anyhow!("mcp server '{}': secret store task failed: {e}", entry.name))?
        .map_err(|e| {
            anyhow::anyhow!(
                "mcp server '{}': failed to load the secret store: {e}",
                entry.name
            )
        })?;
        let expand = |map: &std::collections::BTreeMap<String, String>, kind: &str| {
            map.iter()
                .map(|(k, v)| {
                    let resolved = match v.strip_prefix("secret:") {
                        Some(name) => store.get(name).map(String::from).ok_or_else(|| {
                            anyhow::anyhow!(
                                "mcp server '{}' {kind} '{k}' references no secret named '{name}'",
                                entry.name
                            )
                        })?,
                        None => v.clone(),
                    };
                    Ok((k.clone(), resolved))
                })
                .collect::<Result<std::collections::BTreeMap<_, _>, anyhow::Error>>()
        };
        let mut resolved = entry.clone();
        resolved.env = expand(&entry.env, "env var")?;
        resolved.headers = expand(&entry.headers, "header")?;
        Ok(std::borrow::Cow::Owned(resolved))
    }

    pub fn set_reserved_tool_names(&mut self, names: impl IntoIterator<Item = String>) {
        self.reserved_tool_names = names.into_iter().collect();
        for server in self
            .servers
            .values()
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

    fn is_active(&self, name: &str) -> bool {
        self.servers
            .get(name)
            .is_some_and(|s| matches!(s.status, McpStatus::Running | McpStatus::Pending))
    }

    fn track_pending(&mut self, entry: &McpServerEntry) {
        self.servers.insert(
            entry.name.clone(),
            TrackedServer {
                name: entry.name.clone(),
                command: entry.command.clone(),
                args: entry.args.clone(),
                status: McpStatus::Pending,
                client: None,
                tools: Vec::new(),
            },
        );
    }

    pub(crate) fn reconcile(&mut self, desired: &[McpServerEntry]) -> McpReconcileResult {
        let mut result = McpReconcileResult::default();

        for entry in desired {
            if !self.is_active(&entry.name) {
                result.to_start.push(entry.clone());
                self.track_pending(entry);
            }
        }

        let desired_names: BTreeSet<&str> = desired.iter().map(|e| e.name.as_str()).collect();
        result.to_stop = self
            .servers
            .values()
            .filter(|s| !desired_names.contains(s.name.as_str()))
            .filter(|s| s.status == McpStatus::Running || s.status == McpStatus::Pending)
            .map(|s| s.name.clone())
            .collect();
        result
    }

    fn mark_failed_if_tracked(&mut self, name: &str, reason: &str) {
        if let Some(server) = self.servers.get_mut(name) {
            server.status = McpStatus::Failed(reason.to_string());
        }
    }

    #[tracing::instrument(skip_all, fields(mcp.server = %entry.name))]
    pub async fn connect(&mut self, entry: &McpServerEntry) -> Result<(), anyhow::Error> {
        tracing::debug!("attempting mcp server connection");
        let tools_path = match &self.tools_path {
            Some(handle) => handle.read().await.clone(),
            None => None,
        };
        let secret_resolved = match self.resolve_secrets(entry).await {
            Ok(r) => r,
            Err(e) => {
                self.mark_failed_if_tracked(&entry.name, &e.to_string());
                return Err(e);
            }
        };
        let resolved = match self.resolve_agent_keys(&secret_resolved).await {
            Ok(r) => r,
            Err(e) => {
                self.mark_failed_if_tracked(&entry.name, &e.to_string());
                return Err(e);
            }
        };
        let client = match McpClient::connect(
            &resolved,
            tools_path.as_deref(),
            self.workspace_root.as_deref(),
        )
        .await
        {
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

        self.warn_shadowed_tools(&entry.name, &tools);

        if let Some(server) = self.servers.get_mut(&entry.name) {
            server.status = McpStatus::Running;
            server.client = Some(client);
            server.tools = in_name_order(tools);
            tracing::info!(tool_count = server.tools.len(), "mcp server connected");
        } else {
            tracing::warn!(
                mcp.server = %entry.name,
                "mcp server connected but was removed from tracking before state could be updated — client discarded"
            );
        }

        Ok(())
    }

    #[tracing::instrument(skip_all, fields(mcp.server = %name))]
    pub async fn disconnect(&mut self, name: &str) {
        if let Some(server) = self.servers.remove(name) {
            if let Some(client) = server.client {
                client.shutdown().await;
            }
            tracing::info!("mcp server disconnected");
        }
    }

    #[tracing::instrument(skip_all)]
    pub async fn disconnect_all(&mut self) -> Vec<String> {
        let servers = std::mem::take(&mut self.servers);
        let mut names = Vec::with_capacity(servers.len());

        for (name, server) in servers {
            if let Some(c) = server.client {
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

    #[tracing::instrument(skip_all, fields(server_count = entries.len()))]
    pub async fn connect_servers(&mut self, entries: &[McpServerEntry]) -> McpReconcileReport {
        let mut report = McpReconcileReport::default();

        for entry in entries {
            if self.is_active(&entry.name) {
                continue;
            }
            self.track_pending(entry);
            self.attempt_connect(entry, &mut report).await;
        }

        report
    }

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

    fn warn_shadowed_tools(&self, server_name: &str, tools: &[ToolDefinition]) {
        for tool in tools {
            if self.reserved_tool_names.contains(&tool.name) {
                tracing::warn!(
                    tool = %tool.name,
                    mcp.server = %server_name,
                    "mcp tool name collides with a built-in tool; the built-in wins and the mcp tool is shadowed (unreachable)"
                );
                continue;
            }
            for other in self
                .servers
                .values()
                .filter(|s| s.status == McpStatus::Running && s.name != server_name)
                .filter(|s| s.tools.iter().any(|t| t.name == tool.name))
            {
                let (winner, shadowed) = if other.name.as_str() < server_name {
                    (other.name.as_str(), server_name)
                } else {
                    (server_name, other.name.as_str())
                };
                tracing::warn!(
                    tool = %tool.name,
                    mcp.server = %shadowed,
                    winner = %winner,
                    "mcp tool name collides with a tool from another mcp server; the server whose name sorts first wins and the other is shadowed (unreachable)"
                );
            }
        }
    }

    fn running_owner_of_tool(&self, tool_name: &str) -> Option<&str> {
        self.servers
            .values()
            .filter(|s| s.status == McpStatus::Running)
            .find(|s| s.tools.iter().any(|t| t.name == tool_name))
            .map(|s| s.name.as_str())
    }

    #[must_use]
    pub fn server_name_for_tool(&self, tool_name: &str) -> Option<String> {
        if self.reserved_tool_names.contains(tool_name) {
            return None;
        }
        self.running_owner_of_tool(tool_name).map(str::to_string)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_running_server(&mut self, name: &str, tools: Vec<ToolDefinition>) {
        self.servers.insert(
            name.to_string(),
            TrackedServer {
                name: name.to_string(),
                command: "test".to_string(),
                args: vec![],
                status: McpStatus::Running,
                client: None,
                tools: in_name_order(tools),
            },
        );
    }

    #[must_use]
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut defs: Vec<ToolDefinition> = Vec::new();
        for server in self
            .servers
            .values()
            .filter(|s| s.status == McpStatus::Running)
        {
            for tool in &server.tools {
                if self.reserved_tool_names.contains(&tool.name) {
                    continue;
                }
                if seen.insert(tool.name.as_str()) {
                    defs.push(tool.clone());
                }
            }
        }
        defs
    }

    pub fn resolve_tool(&self, name: &str) -> Result<McpClientHandle, ToolError> {
        let server = self
            .servers
            .values()
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
        Ok(client.handle())
    }

    #[tracing::instrument(skip_all, fields(mcp.tool = %name))]
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<ToolResult, ToolError> {
        self.resolve_tool(name)?.call_tool(name, args).await
    }

    #[cfg(test)]
    pub(crate) fn mark_running(&mut self, name: &str) {
        if let Some(server) = self.servers.get_mut(name) {
            server.status = McpStatus::Running;
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_stopped(&mut self, name: &str) {
        self.servers.remove(name);
    }

    #[cfg(test)]
    pub(crate) fn mark_failed(&mut self, name: &str, reason: &str) {
        if let Some(server) = self.servers.get_mut(name) {
            server.status = McpStatus::Failed(reason.to_string());
        }
    }

    #[cfg(test)]
    pub(crate) fn stop_all(&mut self) -> Vec<String> {
        let names: Vec<String> = self
            .servers
            .values()
            .filter(|s| s.status == McpStatus::Running || s.status == McpStatus::Pending)
            .map(|s| s.name.clone())
            .collect();

        self.servers.clear();
        names
    }

    #[must_use]
    pub fn servers(&self) -> Vec<McpServerState> {
        self.servers.values().map(TrackedServer::snapshot).collect()
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
            env: std::collections::BTreeMap::new(),
            transport: crate::types::McpTransport::default(),
            headers: std::collections::BTreeMap::new(),
            timeout_secs: None,
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

        let initial = vec![entry("fs", "mcp-fs"), entry("git", "mcp-git")];
        registry.reconcile(&initial);
        registry.mark_running("fs");
        registry.mark_running("git");

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

    #[tokio::test]
    async fn reconcile_and_connect_leaves_an_unchanged_running_server_alone() {
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "fs", &["read_file"]);

        let report = registry
            .reconcile_and_connect(&[entry("fs", "mcp-fs")])
            .await;

        assert_eq!(
            report.started, 0,
            "an unchanged server is not connected again"
        );
        assert_eq!(report.stopped, 0, "an unchanged server is not stopped");
        assert!(
            report.failures.is_empty(),
            "an unchanged server is not connected again: {:?}",
            report.failures
        );
        let server = registry
            .servers()
            .into_iter()
            .find(|s| s.name == "fs")
            .unwrap();
        assert_eq!(server.status, McpStatus::Running);
        let names: Vec<&str> = server.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["read_file"], "its tools are kept");
    }

    #[test]
    fn reconcile_skips_already_pending() {
        let mut registry = McpRegistry::new();
        registry.reconcile(&[entry("fs", "mcp-fs")]);

        let result = registry.reconcile(&[entry("fs", "mcp-fs")]);
        assert!(
            result.to_start.is_empty(),
            "should not restart pending server"
        );
        assert!(result.to_stop.is_empty(), "nothing to stop");
    }

    #[test]
    fn a_restarted_server_keeps_its_name_ordered_position() {
        let mut registry = McpRegistry::new();
        let desired = [entry("alpha", "a"), entry("beta", "b"), entry("gamma", "g")];
        registry.reconcile(&desired);
        for name in ["alpha", "beta", "gamma"] {
            registry.mark_running(name);
        }
        registry.mark_failed("beta", "crashed");

        let result = registry.reconcile(&desired);
        assert_eq!(result.to_start.len(), 1, "the failed server restarts");
        registry.mark_running("beta");

        let names: Vec<String> = registry.servers().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
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

    fn push_running_server(registry: &mut McpRegistry, name: &str, tools: &[&str]) {
        registry.insert_running_server(name, tools.iter().map(|t| tool_def(t)).collect());
    }

    #[test]
    fn tool_definitions_follow_server_name_order_not_insertion_order() {
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "zeta", &["z_tool"]);
        push_running_server(&mut registry, "alpha", &["a_tool"]);

        let names: Vec<String> = registry
            .tool_definitions()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["a_tool", "z_tool"]);
    }

    #[test]
    fn a_servers_tools_are_offered_in_name_order_whatever_order_it_lists_them() {
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "srv", &["write_file", "exec", "read_file"]);

        let names: Vec<String> = registry
            .tool_definitions()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(
            names,
            ["exec", "read_file", "write_file"],
            "a server's listing order must not reach the model request"
        );
    }

    #[test]
    fn tool_definitions_dedupes_mcp_collisions_lowest_name_wins() {
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "second", &["shared", "only_second"]);
        push_running_server(&mut registry, "first", &["shared", "only_first"]);

        let defs = registry.tool_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

        assert_eq!(
            names.iter().filter(|n| **n == "shared").count(),
            1,
            "colliding name should appear exactly once"
        );
        assert_eq!(
            registry.server_name_for_tool("shared").as_deref(),
            Some("first"),
            "the server whose name sorts first owns the colliding name"
        );
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
    async fn call_tool_routes_collision_to_lowest_name_server() {
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "first", &["shared"]);
        push_running_server(&mut registry, "second", &["shared"]);

        let err = registry
            .call_tool("shared", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ToolError::Execution(msg) if msg.contains("first")),
            "call should route to the server whose name sorts first, got: {err:?}"
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

    #[test]
    fn resolve_tool_not_found() {
        let registry = McpRegistry::new();
        let err = registry.resolve_tool("nonexistent").unwrap_err();
        assert_eq!(err, ToolError::NotFound("nonexistent".to_string()));
    }

    #[test]
    fn resolve_tool_routes_collision_to_lowest_name_server() {
        let mut registry = McpRegistry::new();
        push_running_server(&mut registry, "first", &["shared"]);
        push_running_server(&mut registry, "second", &["shared"]);

        let err = registry.resolve_tool("shared").unwrap_err();
        assert!(
            matches!(&err, ToolError::Execution(msg) if msg.contains("first")),
            "should resolve to the server whose name sorts first, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn connect_servers_does_not_remove_existing() {
        let mut registry = McpRegistry::new();

        registry.reconcile(&[entry("fs", "mcp-fs"), entry("git", "mcp-git")]);
        registry.mark_running("fs");
        registry.mark_running("git");

        let report = registry
            .connect_servers(&[entry("new-server", "/nonexistent")])
            .await;

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

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_spawns_stdio_server_in_the_configured_workspace_root() {
        // Not a real MCP server: a shell that writes its own cwd to a file
        // and exits (so `connect` reports a failed handshake), but by then it
        // has already written the file this test checks — proving the
        // registry threads its configured workspace root through to the
        // spawned process rather than leaving it on the process cwd. It is
        // the shell itself, not a script written here: executing a file this
        // process just wrote fails with "Text file busy" when a test on
        // another thread forks while the file is still open for writing,
        // because the forked child holds a copy of the descriptor until it
        // execs.
        let dir =
            std::env::temp_dir().join(format!("residuum-mcp-registry-cwd-{}", std::process::id()));
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let pwd_file = dir.join("pwd.txt");

        let mut registry = McpRegistry {
            workspace_root: Some(workspace.clone()),
            ..McpRegistry::new()
        };
        let mut fake = entry("fake", "/bin/sh");
        fake.args = vec![
            "-c".to_string(),
            "pwd > \"$1\"".to_string(),
            "sh".to_string(),
            pwd_file.to_str().unwrap().to_string(),
        ];
        let report = registry.connect_servers(&[fake]).await;
        assert_eq!(
            report.failures.len(),
            1,
            "the fake process isn't a real mcp server, so the handshake should fail"
        );

        let printed = std::fs::canonicalize(std::fs::read_to_string(&pwd_file).unwrap().trim())
            .unwrap_or_else(|e| panic!("child should have written its cwd to {pwd_file:?}: {e}"));
        let expected = std::fs::canonicalize(&workspace).unwrap();
        assert_eq!(
            printed, expected,
            "the spawned server should start in the registry's configured workspace root"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn connect_servers_skips_already_running() {
        let mut registry = McpRegistry::new();

        registry.reconcile(&[entry("fs", "mcp-fs")]);
        registry.mark_running("fs");

        let report = registry.connect_servers(&[entry("fs", "mcp-fs")]).await;

        assert_eq!(report.started, 0, "should not re-connect running server");
        assert!(report.failures.is_empty(), "no failures expected");
        assert_eq!(report.stopped, 0, "nothing stopped");

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

        let servers = registry.servers();
        let server = servers.first().unwrap();
        assert!(
            matches!(server.status, McpStatus::Failed(_)),
            "server should be marked failed"
        );
    }

    #[tokio::test]
    async fn agent_key_references_resolve_from_the_agent_store() {
        let dir = tempfile::tempdir().unwrap();
        let keys = residuum_config::agent_keys::AgentKeys::new_shared(dir.path());
        keys.set(
            "gh",
            "ghp_mcp_value_1",
            None,
            residuum_config::agent_keys::KeyCreator::User,
        )
        .await
        .unwrap();
        let registry = McpRegistry {
            agent_keys: Some(keys),
            ..McpRegistry::new()
        };

        let mut with_ref = entry("gh-server", "gh-mcp");
        with_ref
            .env
            .insert("GITHUB_TOKEN".to_string(), "${agent-key:gh}".to_string());
        let resolved = registry.resolve_agent_keys(&with_ref).await.unwrap();
        assert_eq!(
            resolved.env.get("GITHUB_TOKEN").map(String::as_str),
            Some("ghp_mcp_value_1"),
            "env reference should resolve to the key value"
        );

        let mut missing = entry("broken", "x");
        missing.headers.insert(
            "Authorization".to_string(),
            "Bearer ${agent-key:nope}".to_string(),
        );
        let err = registry.resolve_agent_keys(&missing).await.unwrap_err();
        assert!(
            err.to_string().contains("no agent key named 'nope'"),
            "unknown key should fail the connection visibly: {err}"
        );
    }

    #[tokio::test]
    async fn entries_without_references_skip_the_key_store() {
        let registry = McpRegistry::new();
        let plain = entry("plain", "x");
        let resolved = registry.resolve_agent_keys(&plain).await.unwrap();
        assert!(
            matches!(resolved, std::borrow::Cow::Borrowed(_)),
            "an entry without references must not need a key store"
        );
    }

    #[tokio::test]
    async fn secret_references_resolve_from_the_hub_secret_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = residuum_config::secrets::SecretStore::load(dir.path()).unwrap();
        store
            .set("mcp_tavily_api_key", "tvly-real-value", dir.path())
            .unwrap();
        let registry = McpRegistry {
            secrets_dir: Some(dir.path().to_path_buf()),
            ..McpRegistry::new()
        };

        let mut with_ref = entry("tavily", "tavily-mcp");
        with_ref.env.insert(
            "TAVILY_API_KEY".to_string(),
            "secret:mcp_tavily_api_key".to_string(),
        );
        let resolved = registry.resolve_secrets(&with_ref).await.unwrap();
        assert_eq!(
            resolved.env.get("TAVILY_API_KEY").map(String::as_str),
            Some("tvly-real-value"),
            "env reference should resolve to the stored secret value"
        );

        let mut with_header_ref = entry("remote", "x");
        with_header_ref.headers.insert(
            "Authorization".to_string(),
            "secret:mcp_tavily_api_key".to_string(),
        );
        let resolved_header = registry.resolve_secrets(&with_header_ref).await.unwrap();
        assert_eq!(
            resolved_header
                .headers
                .get("Authorization")
                .map(String::as_str),
            Some("tvly-real-value"),
            "header reference should resolve to the stored secret value too"
        );

        let mut missing = entry("broken", "x");
        missing
            .env
            .insert("TOKEN".to_string(), "secret:nope".to_string());
        let err = registry.resolve_secrets(&missing).await.unwrap_err();
        assert!(
            err.to_string().contains("no secret named 'nope'"),
            "unknown secret should fail the connection visibly: {err}"
        );
    }

    #[tokio::test]
    async fn entries_without_secret_references_skip_the_secret_store() {
        let registry = McpRegistry::new();
        let plain = entry("plain", "x");
        let resolved = registry.resolve_secrets(&plain).await.unwrap();
        assert!(
            matches!(resolved, std::borrow::Cow::Borrowed(_)),
            "an entry without secret references must not need a secret store"
        );
    }

    #[tokio::test]
    async fn secret_reference_with_no_store_configured_fails_visibly() {
        let registry = McpRegistry::new();
        let mut with_ref = entry("tavily", "tavily-mcp");
        with_ref
            .env
            .insert("TAVILY_API_KEY".to_string(), "secret:anything".to_string());
        let err = registry.resolve_secrets(&with_ref).await.unwrap_err();
        assert!(
            err.to_string().contains("no secret store is available"),
            "a bare registry (no secrets_dir) should fail a secret reference clearly: {err}"
        );
    }
}
