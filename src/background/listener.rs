//! Bus listener that turns spawn requests into running sessions.
//!
//! Subscribes to the `Background` topic and, for each `SpawnRequestEvent`,
//! builds isolated resources and hands the run to `SessionRuntime`.
//! Everything the session runs with — model tier, skill, identity — comes
//! from the request itself; there is no resolution step in between.

use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::background::registry::{SessionCategory, SessionState};
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

/// Handle a single spawn request: guard against a live or tearing-down
/// address, then build resources and fork the session.
///
/// Two runs must never be live at the same address at once (see
/// `SessionRegistry::deliver` and `AgentMessenger::send`, which already wait
/// for a `completing` target to clear before publishing a resume). This is
/// the last line of defense against that: a request for an address that is
/// still `forking`/`running`/`idle` is refused outright (that should only
/// happen if a caller races the registry itself, since address generation
/// and `message_agent`'s own resume path both avoid it); a request for one
/// that is `completing` is deferred, off the listener's own task so it
/// doesn't block other addresses' spawns, until the address clears.
async fn handle_spawn_request(
    ctx: &Arc<SpawnContext>,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    if let Some(existing) = ctx.session_registry.get(&event.address) {
        match existing.state {
            SessionState::Completing => {
                tracing::info!(
                    address = %event.address,
                    "spawn target is completing; deferring until it clears the registry"
                );
                let ctx = Arc::clone(ctx);
                tokio::spawn(async move {
                    ctx.session_registry.wait_until_clear(&event.address).await;
                    if let Err(e) = fork_and_spawn(&ctx, event).await {
                        tracing::warn!(error = %e, "failed to fork deferred session resume");
                    }
                });
                return Ok(());
            }
            SessionState::Forking | SessionState::Running | SessionState::Idle => {
                tracing::error!(
                    address = %event.address,
                    state = %existing.state,
                    "refusing to fork a session at an address that is already live"
                );
                anyhow::bail!(
                    "session address {} is already live, refusing spawn/resume",
                    event.address
                );
            }
            SessionState::Completed => {}
        }
    }
    fork_and_spawn(ctx, event).await
}

/// Build resources and hand the run to `SessionRuntime`, once
/// `handle_spawn_request` has confirmed the address is free to take it.
async fn fork_and_spawn(
    ctx: &Arc<SpawnContext>,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    let skill = event.skill.as_ref().map(|s| s.as_ref().to_string());
    let category = SessionCategory::from_trigger(&event.source);

    let resources = build_spawn_resources(
        ctx,
        &event.model_tier,
        skill.as_deref(),
        &event.address,
        category,
    )
    .await?;

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
