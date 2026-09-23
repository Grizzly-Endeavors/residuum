//! Session management tools: `stop_agent`, `list_agents`, and `subagent_spawn`.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

use crate::a2a::{A2aClientHub, AgentSource, AgentStatus, RemoteTaskTracker};
use crate::agent::HopCounter;
use crate::background::registry::{MAIN_ADDRESS, SessionRegistry, generate_address};
use crate::bus::{EventTrigger, SessionAddress};
use crate::config::BackgroundModelTier;
use crate::inference::ToolDefinition;
use crate::skills::SharedSkillState;

use super::{Tool, ToolError, ToolResult};

// ─── StopAgentTool ───────────────────────────────────────────────────────────

/// Tool for stopping a live session by address, or an open remote A2A task
/// (`a2a:<name>`).
pub struct StopAgentTool {
    registry: Arc<SessionRegistry>,
    /// This agent's own address, used to look up its own open remote task
    /// with `a2a:<name>`.
    self_address: SessionAddress,
    a2a_hub: Arc<A2aClientHub>,
    a2a_tracker: Arc<RemoteTaskTracker>,
}

impl StopAgentTool {
    /// Create a new `StopAgentTool`.
    #[must_use]
    pub fn new(
        registry: Arc<SessionRegistry>,
        self_address: SessionAddress,
        a2a_hub: Arc<A2aClientHub>,
        a2a_tracker: Arc<RemoteTaskTracker>,
    ) -> Self {
        Self {
            registry,
            self_address,
            a2a_hub,
            a2a_tracker,
        }
    }

    async fn stop_remote_agent(&self, agent_name: &str) -> Result<ToolResult, ToolError> {
        if !self.a2a_hub.agent_exists(agent_name).await {
            return Ok(ToolResult::error(format!(
                "no remote agent named 'a2a:{agent_name}'. Check config/a2a.json or list_agents."
            )));
        }
        let sender = self.self_address.as_ref();
        match self.a2a_tracker.cancel_open_task(sender, agent_name).await {
            Ok(Some(task_id)) => Ok(ToolResult::success(format!(
                "Canceling task {task_id} with remote agent a2a:{agent_name}."
            ))),
            Ok(None) => Ok(ToolResult::error(format!(
                "no open task with remote agent a2a:{agent_name}."
            ))),
            Err(e) => Ok(ToolResult::error(e.to_string())),
        }
    }
}

#[async_trait]
impl Tool for StopAgentTool {
    fn name(&self) -> &'static str {
        "stop_agent"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Stop a live session by address, or cancel your open task with a \
                          remote agent (address \"a2a:<name>\"). Stopping a session cancels any \
                          in-flight turn and moves it to completing; its transcript is kept, not \
                          discarded. The main agent cannot be stopped this way. Use list_agents \
                          to find live addresses and remote agents."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "address": {
                        "type": "string",
                        "description": "The address of the session to stop, or \"a2a:<name>\" to cancel your open task with that remote agent"
                    }
                },
                "required": ["address"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let address = super::require_str(&arguments, "address")?;

        if address == MAIN_ADDRESS {
            return Err(ToolError::InvalidArguments(
                "the main agent cannot be stopped with stop_agent".to_string(),
            ));
        }

        if let Some(agent_name) = address.strip_prefix("a2a:") {
            return self.stop_remote_agent(agent_name).await;
        }

        if self.registry.stop(&SessionAddress::from(address)) {
            Ok(ToolResult::success(format!("Stopping session {address}.")))
        } else {
            Ok(ToolResult::error(format!(
                "No live session with address {address}."
            )))
        }
    }
}

// ─── ListAgentsTool ──────────────────────────────────────────────────────────

/// Tool for listing the main agent, every live session, and every remote A2A
/// agent.
pub struct ListAgentsTool {
    registry: Arc<SessionRegistry>,
    self_address: SessionAddress,
    a2a_hub: Arc<A2aClientHub>,
    a2a_tracker: Arc<RemoteTaskTracker>,
}

impl ListAgentsTool {
    /// Create a new `ListAgentsTool`.
    #[must_use]
    pub fn new(
        registry: Arc<SessionRegistry>,
        self_address: SessionAddress,
        a2a_hub: Arc<A2aClientHub>,
        a2a_tracker: Arc<RemoteTaskTracker>,
    ) -> Self {
        Self {
            registry,
            self_address,
            a2a_hub,
            a2a_tracker,
        }
    }
}

#[async_trait]
impl Tool for ListAgentsTool {
    fn name(&self) -> &'static str {
        "list_agents"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "List the main agent, every live (running or idle) session, and every \
                          remote agent reachable over A2A (address \"a2a:<name>\"): for sessions, \
                          address, category, source, state, depth, spawner, elapsed time, and \
                          purpose; for remote agents, online status, description, skills, and \
                          your own open tasks with them. Completed sessions are not listed, but \
                          their addresses remain valid."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
        let sessions = self.registry.list_live();
        let now = Utc::now();

        let mut lines = vec![
            "main — always live".to_string(),
            format!("{} live session(s):", sessions.len()),
        ];

        for info in &sessions {
            let elapsed_secs = (now - info.started_at).num_seconds().max(0);
            let spawner = info
                .spawner
                .as_ref()
                .map_or_else(|| "-".to_string(), ToString::to_string);
            lines.push(format!(
                "  [{address}] {source} — category: {category} — state: {state} — depth: {depth} \
                 — spawner: {spawner} — running {elapsed}s — purpose: {purpose}",
                address = info.address,
                source = info.source_label,
                category = info.category,
                state = info.state,
                depth = info.depth,
                elapsed = elapsed_secs,
                purpose = info.purpose,
            ));
        }

        let agents = self.a2a_hub.snapshot().await;
        let open_tasks = self
            .a2a_tracker
            .open_tasks_for(self.self_address.as_ref())
            .await;
        lines.push(String::new());
        lines.push(format!("{} remote agent(s):", agents.len()));
        for agent in &agents {
            let status = match &agent.status {
                AgentStatus::Pending => "resolving its agent card".to_string(),
                AgentStatus::Ok(card) => format!("online — {}", card.description),
                AgentStatus::Error(e) => format!("error — {e}"),
            };
            let instance_note = if agent.source == AgentSource::Sibling {
                " (your instance)"
            } else {
                ""
            };
            let mut line = format!("  [a2a:{}]{instance_note} {status}", agent.name);
            if let Some(card) = agent.card()
                && !card.skills.is_empty()
            {
                let skills: Vec<String> = card
                    .skills
                    .iter()
                    .map(|s| format!("{} ({})", s.name, s.id))
                    .collect();
                write!(line, " — skills: {}", skills.join(", ")).ok();
            }
            lines.push(line);
            for task in open_tasks.iter().filter(|t| t.agent == agent.name) {
                lines.push(format!(
                    "    task {} — {} — {}",
                    task.task_id,
                    task.state,
                    task.last_status_text
                        .as_deref()
                        .unwrap_or("(no status yet)")
                ));
            }
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ─── SubagentSpawnTool ──────────────────────────────────────────────────────

/// Tool for forking sessions on demand.
pub struct SubagentSpawnTool {
    publisher: crate::bus::Publisher,
    /// Main agent skill state — read to validate a requested skill name.
    skill_state: SharedSkillState,
    /// The caller's own address, recorded as the spawner of any session this
    /// tool forks (`main`, or a session's own address).
    spawner_address: SessionAddress,
    /// The caller's own depth from the main agent (main = 0).
    depth: u32,
    /// Maximum depth a spawned session may have. Spawning is refused once
    /// `depth + 1` would exceed this.
    depth_cap: u32,
    /// This agent's current-turn hop counter — the new session's first turn
    /// carries one more than the highest hop count among the inputs driving
    /// this (the spawning) turn.
    hop_counter: HopCounter,
}

impl SubagentSpawnTool {
    /// Create a new `SubagentSpawnTool`.
    #[must_use]
    pub(crate) fn new(
        publisher: crate::bus::Publisher,
        skill_state: SharedSkillState,
        spawner_address: SessionAddress,
        depth: u32,
        depth_cap: u32,
        hop_counter: HopCounter,
    ) -> Self {
        Self {
            publisher,
            skill_state,
            spawner_address,
            depth,
            depth_cap,
            hop_counter,
        }
    }
}

#[async_trait]
impl Tool for SubagentSpawnTool {
    fn name(&self) -> &'static str {
        "subagent_spawn"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Fork a session to handle a task in the background. Optionally name a \
                          skill to give the session a role — its instructions become the \
                          session's brief. Runs asynchronously; each turn's result is relayed \
                          back to you tagged with the session's address. Returns the session's \
                          address immediately — use it with list_agents or stop_agent. A \
                          session's result is its own self-report, not verified fact — for \
                          verifiable work, ask it to return concrete handles (file paths, IDs, \
                          URLs) and verify them yourself before relying on the result."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The prompt/instructions for the session"
                    },
                    "skill": {
                        "type": "string",
                        "description": "Name of a skill to activate for the session, giving it a role. Omit to run on the task prompt alone."
                    },
                    "model": {
                        "type": "string",
                        "enum": ["small", "medium", "large"],
                        "description": "Model tier for the session (default: \"medium\")."
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let task_prompt = super::require_str(&arguments, "task")?;

        if task_prompt.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "task must not be empty".to_string(),
            ));
        }

        let skill_name = arguments.get("skill").and_then(Value::as_str);

        if let Some(name) = skill_name {
            if name.eq_ignore_ascii_case("main") {
                return Err(ToolError::InvalidArguments(
                    "\"main\" is reserved; name a skill instead, or omit skill to run on the \
                     task prompt alone."
                        .to_string(),
                ));
            }

            // Validate against the in-memory skill index so an unknown name fails
            // here, rather than surfacing later as a failed session result.
            let state = self.skill_state.lock().await;
            if state.index().find_by_name(name).is_none() {
                let available: Vec<&str> = state
                    .index()
                    .entries()
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect();
                return Ok(ToolResult::error(format!(
                    "unknown skill '{name}'. Available: {}",
                    available.join(", ")
                )));
            }
        }

        let model_tier = match arguments.get("model").and_then(Value::as_str) {
            Some(s) => parse_model_tier(s)?,
            None => BackgroundModelTier::Medium,
        };

        let new_depth = self.depth + 1;
        if new_depth > self.depth_cap {
            return Ok(ToolResult::error(format!(
                "cannot spawn: nesting depth cap ({}) reached at depth {} — handle this task \
                 directly instead of spawning further, or have a shallower agent spawn it",
                self.depth_cap, self.depth
            )));
        }

        let trigger = EventTrigger::Agent;
        let address = generate_address(&trigger, skill_name.unwrap_or("subagent"));

        let spawn_event = crate::bus::SpawnRequestEvent {
            address: address.clone(),
            skill: skill_name.map(crate::bus::SkillName::from),
            source_label: format!("agent:{}", skill_name.unwrap_or("subagent")),
            prompt: task_prompt.to_string(),
            context: None,
            source: trigger,
            model_tier,
            spawner: Some(self.spawner_address.clone()),
            depth: new_depth,
            hop_count: self.hop_counter.outgoing(),
            sender: None,
            conversation: None,
            inbound: None,
            images: Vec::new(),
        };

        self.publisher
            .publish(crate::bus::topics::Background, spawn_event)
            .await
            .map_err(|err| {
                tracing::error!(error = %err, skill = skill_name.unwrap_or("none"), "failed to publish spawn request");
                ToolError::Execution(format!("failed to publish spawn request: {err}"))
            })?;

        Ok(ToolResult::success(match skill_name {
            Some(name) => format!("Session {address} spawned with skill '{name}'."),
            None => format!("Session {address} spawned."),
        }))
    }
}

fn parse_model_tier(s: &str) -> Result<BackgroundModelTier, ToolError> {
    s.parse::<BackgroundModelTier>()
        .map_err(ToolError::InvalidArguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillIndex, SkillState};

    fn make_tool() -> SubagentSpawnTool {
        make_tool_with_depth(MAIN_ADDRESS, 0, 2)
    }

    fn make_tool_with_depth(spawner: &str, depth: u32, depth_cap: u32) -> SubagentSpawnTool {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        SubagentSpawnTool::new(
            publisher,
            skill_state,
            SessionAddress::from(spawner),
            depth,
            depth_cap,
            HopCounter::new(0),
        )
    }

    #[test]
    fn model_tier_parsing_valid() {
        assert!(matches!(
            parse_model_tier("small"),
            Ok(BackgroundModelTier::Small)
        ));
        assert!(matches!(
            parse_model_tier("medium"),
            Ok(BackgroundModelTier::Medium)
        ));
        assert!(matches!(
            parse_model_tier("large"),
            Ok(BackgroundModelTier::Large)
        ));
    }

    #[test]
    fn model_tier_parsing_invalid() {
        assert!(parse_model_tier("invalid").is_err());
        assert!(parse_model_tier("SMALL").is_err());
    }

    #[tokio::test]
    async fn task_required() {
        let tool = make_tool();

        // Missing task
        let missing_result = tool.execute(serde_json::json!({})).await;
        assert!(missing_result.is_err(), "should error on missing task");

        // Empty task
        let empty_result = tool.execute(serde_json::json!({"task": "  "})).await;
        assert!(empty_result.is_err(), "should error on empty task");
    }

    #[tokio::test]
    async fn main_skill_name_rejected() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "skill": "main"
            }))
            .await;

        assert!(result.is_err(), "\"main\" should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("reserved"),
            "error should mention 'reserved', got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn main_skill_name_rejected_case_insensitive() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "skill": "MAIN"
            }))
            .await;

        assert!(result.is_err(), "\"MAIN\" should also be rejected");
    }

    #[tokio::test]
    async fn unknown_skill_name_returns_error() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "skill": "definitely-not-a-real-skill"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "unknown skill should return a tool error");
        assert!(
            result.output.contains("unknown skill"),
            "error should mention unknown skill, got: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn spawns_without_a_skill_and_returns_address() {
        // Omitting `skill` spawns a plain session — no index lookup, no error.
        let tool = make_tool();

        let res = tool
            .execute(serde_json::json!({
                "task": "do something"
            }))
            .await
            .unwrap();

        assert!(
            !res.is_error,
            "spawning without a skill should succeed, got: {}",
            res.output
        );
        assert!(
            res.output.contains("spawned-subagent-"),
            "success message should include the generated session address, got: {}",
            res.output
        );
    }

    #[tokio::test]
    async fn spawn_with_skill_returns_address_and_skill_name() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("researcher");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: researcher\ndescription: researches things\n---\nBody.",
        )
        .await
        .unwrap();
        let index = SkillIndex::scan(&[dir.path().to_path_buf()]).await.unwrap();

        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let skill_state = SkillState::new_shared(index, vec![dir.path().to_path_buf()]);
        let tool = SubagentSpawnTool::new(
            publisher,
            skill_state,
            SessionAddress::from(MAIN_ADDRESS),
            0,
            2,
            HopCounter::new(0),
        );

        let res = tool
            .execute(serde_json::json!({
                "task": "research the thing",
                "skill": "researcher"
            }))
            .await
            .unwrap();

        assert!(!res.is_error, "got: {}", res.output);
        assert!(res.output.contains("spawned-researcher-"));
        assert!(res.output.contains("researcher"));
    }

    #[tokio::test]
    async fn spawn_at_the_depth_cap_is_refused() {
        // A session at depth 2 with a cap of 2 would create a depth-3 child,
        // which exceeds the cap.
        let tool = make_tool_with_depth("spawned-parent-0001", 2, 2);

        let result = tool
            .execute(serde_json::json!({ "task": "do something" }))
            .await
            .unwrap();

        assert!(result.is_error, "spawning past the cap should be refused");
        assert!(
            result.output.contains("depth cap"),
            "error should explain the depth cap, got: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn spawn_at_depth_below_cap_succeeds() {
        // A session at depth 1 with a cap of 2 creates a depth-2 child, which
        // is exactly at the cap and still allowed.
        let tool = make_tool_with_depth("spawned-parent-0001", 1, 2);

        let result = tool
            .execute(serde_json::json!({ "task": "do something" }))
            .await
            .unwrap();

        assert!(!result.is_error, "got: {}", result.output);
    }

    #[tokio::test]
    async fn spawn_records_the_calling_session_as_spawner_and_its_depth_plus_one() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut subscriber = bus_handle
            .subscribe(crate::bus::topics::Background)
            .await
            .unwrap();
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let tool = SubagentSpawnTool::new(
            publisher,
            skill_state,
            SessionAddress::from("spawned-parent-0001"),
            1,
            2,
            HopCounter::new(3),
        );

        tool.execute(serde_json::json!({ "task": "do something" }))
            .await
            .unwrap();

        let event: crate::bus::SpawnRequestEvent = subscriber.recv().await.unwrap().unwrap();
        assert_eq!(
            event.spawner,
            Some(SessionAddress::from("spawned-parent-0001"))
        );
        assert_eq!(event.depth, 2);
        assert_eq!(
            event.hop_count, 4,
            "the new session's first turn must carry one more than the spawning turn's hop count"
        );
    }

    /// A tracker with no persisted state and an unopened outbound file — safe
    /// to build fresh per test since nothing here touches the real filesystem
    /// beyond a throwaway temp dir the caller keeps alive.
    async fn bare_a2a(dir: &std::path::Path) -> (Arc<A2aClientHub>, Arc<RemoteTaskTracker>) {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let store = Arc::new(crate::background::store::SessionStore::new(
            dir.join("sessions"),
        ));
        let messenger = Arc::new(crate::background::messaging::AgentMessenger::new(
            registry,
            bus_handle.publisher(),
            store,
            crate::background::HopLimits { soft: 8, hard: 32 },
        ));
        let hub = A2aClientHub::new_shared();
        let tracker = RemoteTaskTracker::load(
            dir.join("outbound.json"),
            Arc::clone(&hub),
            messenger,
            dir.join("inbox"),
        )
        .await;
        (hub, tracker)
    }

    #[tokio::test]
    async fn stop_agent_rejects_main_address() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let (hub, tracker) = bare_a2a(dir.path()).await;
        let tool = StopAgentTool::new(registry, SessionAddress::from(MAIN_ADDRESS), hub, tracker);

        let result = tool.execute(serde_json::json!({ "address": "main" })).await;
        assert!(result.is_err(), "stopping main should be rejected");
    }

    #[tokio::test]
    async fn stop_agent_reports_unknown_address() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let (hub, tracker) = bare_a2a(dir.path()).await;
        let tool = StopAgentTool::new(registry, SessionAddress::from(MAIN_ADDRESS), hub, tracker);

        let result = tool
            .execute(serde_json::json!({ "address": "spawned-ghost-0000" }))
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn stop_agent_reports_plain_language_error_for_unknown_remote_agent() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let (hub, tracker) = bare_a2a(dir.path()).await;
        let tool = StopAgentTool::new(registry, SessionAddress::from(MAIN_ADDRESS), hub, tracker);

        let result = tool
            .execute(serde_json::json!({ "address": "a2a:nope" }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("a2a:nope"), "got: {}", result.output);
    }

    #[tokio::test]
    async fn list_agents_always_includes_main() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let (hub, tracker) = bare_a2a(dir.path()).await;
        let tool = ListAgentsTool::new(registry, SessionAddress::from(MAIN_ADDRESS), hub, tracker);

        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("main"));
        assert!(result.output.contains("remote agent(s)"));
    }

    #[tokio::test]
    async fn list_agents_shows_remote_agent_status_and_open_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let (hub, tracker) = bare_a2a(dir.path()).await;
        hub.register_external(
            "laptop".to_string(),
            "http://127.0.0.1:1".to_string(),
            std::collections::HashMap::new(),
            crate::a2a::AgentSource::Config,
        )
        .await;
        tracker
            .track(
                &SessionAddress::from(MAIN_ADDRESS),
                "laptop",
                "task-1".to_string(),
                "ctx-1".to_string(),
                "working",
                0,
            )
            .await;
        let tool = ListAgentsTool::new(registry, SessionAddress::from(MAIN_ADDRESS), hub, tracker);

        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(
            result.output.contains("[a2a:laptop]"),
            "got: {}",
            result.output
        );
        assert!(result.output.contains("task-1"), "got: {}", result.output);
    }

    #[tokio::test]
    async fn list_agents_labels_a_sibling_as_your_instance() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let (hub, tracker) = bare_a2a(dir.path()).await;
        hub.register_external(
            "laptop".to_string(),
            "http://127.0.0.1:1".to_string(),
            std::collections::HashMap::new(),
            AgentSource::Sibling,
        )
        .await;
        hub.register_external(
            "colleague".to_string(),
            "http://127.0.0.1:1".to_string(),
            std::collections::HashMap::new(),
            AgentSource::Config,
        )
        .await;
        let tool = ListAgentsTool::new(registry, SessionAddress::from(MAIN_ADDRESS), hub, tracker);

        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(
            result.output.contains("[a2a:laptop] (your instance)"),
            "got: {}",
            result.output
        );
        assert!(
            !result.output.contains("[a2a:colleague] (your instance)"),
            "a config-sourced agent must not be labeled as the user's own instance: {}",
            result.output
        );
    }
}
