//! Bus listener that turns spawn requests into running sessions.
//!
//! Subscribes to the `Background` topic and, for each `SpawnRequestEvent`,
//! builds isolated resources and hands the run to `SessionRuntime`.
//! Everything the session runs with — model tier, skill, identity — comes
//! from the request itself; there is no resolution step in between.

use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::background::runtime::SessionSpawnRequest;
use crate::background::spawn_context::{SpawnContext, build_spawn_resources};
use crate::background::types::SubAgentConfig;
use crate::bus::{BusHandle, SpawnRequestEvent, Subscriber, topics};

/// Subscribe to the `Background` topic and fork sessions on demand.
///
/// Returns a `JoinHandle` for shutdown coordination. The task runs until the
/// bus shuts down or the subscriber closes.
#[tracing::instrument(skip_all)]
pub(crate) async fn spawn_listener(
    ctx: Arc<SpawnContext>,
    bus_handle: &BusHandle,
) -> Option<JoinHandle<()>> {
    let subscriber: Subscriber<SpawnRequestEvent> = match bus_handle
        .subscribe(topics::Background)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "spawn listener failed to subscribe to Background topic");
            return None;
        }
    };

    tracing::info!("spawn listener subscribed to Background topic");

    Some(tokio::spawn(listener_loop(ctx, subscriber)))
}

/// Main loop: reads spawn requests and executes them.
async fn listener_loop(ctx: Arc<SpawnContext>, mut subscriber: Subscriber<SpawnRequestEvent>) {
    loop {
        match subscriber.recv().await {
            Ok(Some(event)) => {
                let source_label = event.source_label.clone();
                if let Err(e) = handle_spawn_request(&ctx, event).await {
                    tracing::warn!(
                        source = %source_label,
                        error = %e,
                        "failed to fork session"
                    );
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::error!(error = %e, "spawn listener subscriber error, shutting down");
                break;
            }
        }
    }
    tracing::info!("spawn listener shutting down");
}

/// Handle a single spawn request: build resources, fork the session.
async fn handle_spawn_request(
    ctx: &Arc<SpawnContext>,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    let skill = event.skill.as_ref().map(|s| s.as_ref().to_string());

    let resources = build_spawn_resources(ctx, &event.model_tier, skill.as_deref()).await?;

    let request = SessionSpawnRequest {
        address: event.address,
        source_label: event.source_label,
        trigger: event.source,
        agent_skill: event.skill,
        subagent_config: SubAgentConfig {
            prompt: event.prompt,
            context: event.context,
            model_tier: event.model_tier,
        },
    };

    let log_address = request.address.clone();
    let log_source_label = request.source_label.clone();
    ctx.session_runtime.spawn(request, Some(resources));
    tracing::info!(
        skill = skill.as_deref().unwrap_or("none"),
        address = %log_address,
        source = %log_source_label,
        "forked session"
    );
    Ok(())
}
