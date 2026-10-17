//! Agent creation: wraps main's tool registry in the agent that runs its turns.

use std::sync::Arc;

use crate::agent::{Agent, AgentConfig, ToolRegistry};
use crate::config::WorkspaceLayout;
use crate::mcp::SharedMcpRegistry;
use crate::memory::recent_messages::load_messages_for_agent;
use crate::workspace::identity::IdentityFiles;

/// Arguments for creating the agent, bundled to stay under the argument limit.
pub(super) struct CreateAgentArgs {
    pub provider: Box<dyn crate::inference::InferenceProvider>,
    pub options: crate::inference::CompletionOptions,
    /// Maximum tool-call iterations for the agent's turns, from
    /// `cfg.agent.max_tool_iterations`. `None` means unlimited.
    pub max_tool_iterations: Option<usize>,
    /// Guards against a model repeating the exact same tool call, from
    /// `cfg.agent.repeat_call_guard`.
    pub repeat_call_guard: crate::config::RepeatCallGuardConfig,
    pub tools: ToolRegistry,
    pub identity: IdentityFiles,
    /// Main's current-turn hop counter — the same instance already handed to
    /// the `message_agent`/`subagent_spawn` tools in `args.tools` at
    /// registration time, so the agent and its tools always agree on the
    /// current turn's hop count.
    pub hop_counter: crate::agent::HopCounter,
}

/// Create the agent, load observations, recent context, and restore messages.
///
/// Each of the three loads below degrades independently rather than failing
/// agent construction — a missing or corrupt snapshot means the agent starts
/// with less context, not that it fails to start — but each is still folded
/// into `degradations` as a plain-language line for the caller's grouped
/// startup-degradation notice (see `super::degradation_notice`), so a
/// silently emptier agent is still visible to the user.
pub(super) async fn create_agent(
    args: CreateAgentArgs,
    mcp_registry: &SharedMcpRegistry,
    tz: chrono_tz::Tz,
    layout: &WorkspaceLayout,
    degradations: &mut Vec<String>,
) -> Agent {
    let mut agent = Agent::new(
        args.provider,
        args.tools,
        Arc::clone(mcp_registry),
        args.identity,
        AgentConfig {
            options: args.options,
            tz,
            layout: Some(layout.clone()),
        },
        args.hop_counter,
    );
    agent.set_max_tool_iterations(args.max_tool_iterations);
    agent.set_repeat_call_guard(args.repeat_call_guard);
    if let Err(err) = agent.reload_observations(layout).await {
        tracing::warn!(error = %err, "observation loading degraded");
        degradations.push(format!(
            "your past observations couldn't be loaded, so I'm starting this session without them: {err}"
        ));
    }
    if let Err(err) = agent.reload_recent_context(layout).await {
        tracing::warn!(error = %err, "recent context loading degraded");
        degradations.push(format!(
            "your recent context summary couldn't be loaded, so I'm starting this session without it: {err}"
        ));
    }

    match load_messages_for_agent(&layout.recent_messages_json()).await {
        Ok(restore) => {
            if !restore.messages.is_empty() {
                tracing::info!(
                    count = restore.messages.len(),
                    "restoring recent messages from previous run"
                );
                agent.restore_messages(restore.messages);
            }
            agent.set_last_user_message_at(restore.last_user_message_at);
        }
        Err(err) => {
            tracing::warn!(error = %err, "message restore degraded: starting with empty history");
            degradations.push(format!(
                "your recent messages from before this restart couldn't be restored, so I'm starting with empty history: {err}"
            ));
        }
    }

    let usage_totals =
        crate::agent::usage::load_session_usage_totals(&layout.usage_totals_json()).await;
    agent.restore_usage_totals(usage_totals).await;

    agent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::HopCounter;
    use crate::background::team::TeamDirectory as _;
    use crate::tools::testing::RegistryHarness;

    /// A spawned session's `agent_create` reaches the hub as the agent it
    /// belongs to, so a session can create a teammate.
    #[tokio::test]
    async fn a_session_fork_can_create_a_teammate() {
        use crate::hub::test_support::{mount_reply, write_agent};

        let hub_root = tempfile::tempdir().expect("tempdir");
        let hub_dir = hub_root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).expect("hub dir");
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"\n").expect("hub config");
        let model = wiremock::MockServer::start().await;
        mount_reply(&model, "scout here").await;
        write_agent(hub_root.path(), "scout", &model.uri());
        let hub_cfg = crate::config::HubConfig::load_at(&hub_dir).expect("hub config loads");
        let services =
            crate::hub::services::HubServices::for_tests(hub_root.path(), &hub_cfg).await;
        let host = crate::hub::AgentHost::new(
            services.clone(),
            hub_cfg,
            Arc::new(crate::gateway::GatewayLauncher::new()),
        );
        host.discover().expect("discovery");

        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = RegistryHarness::build(dir.path()).await;
        h.lifecycle = crate::tools::LifecycleAccess::new(services.directory.clone(), "scout");
        let session = h.session_registry_for("spawned-test-0001", "spawned", None);

        let result = session
            .execute("agent_create", serde_json::json!({ "name": "nova" }))
            .await
            .expect("agent_create runs in a session");

        assert!(!result.is_error, "got: {}", result.output);
        assert!(
            result.output.contains("agent:nova"),
            "got: {}",
            result.output
        );
        assert!(hub_root.path().join("nova").join("config").is_dir());
        assert!(host.summary("nova").is_ok());
    }

    /// A malformed observations/recent-context/recent-messages snapshot each
    /// degrades `create_agent` independently (the agent still starts) and
    /// each lands its own plain-language line in the caller's grouped
    /// startup-degradation notice, rather than only a log line no one sees.
    #[tokio::test]
    async fn create_agent_degradations_are_collected_for_the_caller() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = WorkspaceLayout::new(dir.path());

        for path in [
            layout.observations_json(),
            layout.recent_context_json(),
            layout.recent_messages_json(),
        ] {
            std::fs::create_dir_all(path.parent().expect("snapshot path has a parent"))
                .expect("failed to create snapshot dir");
            std::fs::write(&path, "not valid json").expect("failed to write malformed snapshot");
        }

        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let mut degradations: Vec<String> = Vec::new();

        let _agent = create_agent(
            CreateAgentArgs {
                provider: Box::new(crate::inference::NullProvider),
                options: crate::inference::CompletionOptions::default(),
                max_tool_iterations: None,
                repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
                tools: crate::agent::ToolRegistry::new(),
                identity: IdentityFiles::default(),
                hop_counter: HopCounter::new(0),
            },
            &mcp_registry,
            chrono_tz::UTC,
            &layout,
            &mut degradations,
        )
        .await;

        assert_eq!(
            degradations.len(),
            3,
            "each of the three malformed snapshots should degrade independently: {degradations:?}"
        );
        assert!(
            degradations.iter().any(|d| d.contains("observations")),
            "missing observations degradation: {degradations:?}"
        );
        assert!(
            degradations.iter().any(|d| d.contains("recent context")),
            "missing recent context degradation: {degradations:?}"
        );
        assert!(
            degradations.iter().any(|d| d.contains("recent messages")),
            "missing recent messages degradation: {degradations:?}"
        );
    }
}
