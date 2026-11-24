use std::sync::Arc;

use residuum_agent::{Agent, AgentConfig, ToolRegistry};
use residuum_config::WorkspaceLayout;
use residuum_mcp::SharedMcpRegistry;
use residuum_memory::recent_messages::load_messages_for_agent;
use residuum_workspace::identity::IdentityFiles;

pub(super) struct CreateAgentArgs {
    pub provider: Box<dyn residuum_inference::InferenceProvider>,
    pub options: residuum_inference::CompletionOptions,
    pub max_tool_iterations: Option<usize>,
    pub repeat_call_guard: residuum_config::RepeatCallGuardConfig,
    pub tools: ToolRegistry,
    pub identity: IdentityFiles,
    pub hop_counter: residuum_agent::HopCounter,
}

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
        residuum_agent::usage::load_session_usage_totals(&layout.usage_totals_json()).await;
    agent.restore_usage_totals(usage_totals).await;

    agent
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_agent::HopCounter;
    use residuum_background::team::TeamDirectory as _;
    use residuum_tools::testing::RegistryHarness;

    #[tokio::test]
    async fn a_session_fork_can_create_a_teammate() {
        use residuum_hub::testing::{mount_reply, write_agent};

        let hub_root = tempfile::tempdir().expect("tempdir");
        let hub_dir = hub_root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).expect("hub dir");
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"\n").expect("hub config");
        let model = wiremock::MockServer::start().await;
        mount_reply(&model, "scout here").await;
        write_agent(hub_root.path(), "scout", &model.uri());
        let hub_cfg = residuum_config::HubConfig::load_at(&hub_dir).expect("hub config loads");
        let services =
            residuum_hub::services::HubServices::for_tests(hub_root.path(), &hub_cfg).await;
        let host = residuum_hub::AgentHost::new(
            services.clone(),
            hub_cfg,
            Arc::new(crate::GatewayLauncher::new()),
        );
        host.discover().expect("discovery");

        let dir = tempfile::tempdir().expect("tempdir");
        let mut h = RegistryHarness::build(dir.path()).await;
        h.lifecycle = residuum_tools::LifecycleAccess::new(services.directory.clone(), "scout");
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

        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
        let mut degradations: Vec<String> = Vec::new();

        let _agent = create_agent(
            CreateAgentArgs {
                provider: Box::new(residuum_inference::NullProvider),
                options: residuum_inference::CompletionOptions::default(),
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                tools: residuum_agent::ToolRegistry::new(),
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
