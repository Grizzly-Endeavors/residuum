//! Bus listener that turns spawn requests into running sessions.
//!
//! Subscribes to the `Background` topic and, for each `SpawnRequestEvent`,
//! builds isolated resources and hands the run to `SessionRuntime`.
//! Everything the session runs with — model tier, skill, identity — comes
//! from the request itself; there is no resolution step in between.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::agent::interrupt::Interrupt;
use crate::background::registry::{DeliverOutcome, MAIN_ADDRESS, SessionCategory, SessionState};
use crate::background::runtime::SessionSpawnRequest;
use crate::background::spawn_context::{SpawnContext, build_spawn_resources};
use crate::background::types::SubAgentConfig;
use crate::bus::{
    AgentMessageEvent, BusHandle, SessionAddress, SpawnRequestEvent, Subscriber, topics,
};

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
/// already `forking`/`running`/`idle` delivers its content into that live
/// run as a message instead of forking a second one — this is the normal
/// (not exceptional) outcome when two messages were queued to the same
/// completing session and both ended up resuming it (see
/// `AgentMessenger::deferred_resume`), since only one of the two resulting
/// spawn requests can actually win the fork. A request for an address that
/// is `completing` is deferred, off the listener's own task so it doesn't
/// block other addresses' spawns, until the address clears.
fn handle_spawn_request<'a>(
    ctx: &'a Arc<SpawnContext>,
    event: SpawnRequestEvent,
) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + 'a>> {
    // Written as a plain fn returning a boxed future, rather than `async
    // fn`, because the `Completing` branch below recurses into this same
    // function (via a detached task) — an `async fn` calling itself, even
    // indirectly through `tokio::spawn`, creates a cyclic `Send`-auto-trait
    // computation rustc cannot resolve on its own; boxing here gives the
    // recursive call a concrete, non-opaque type that breaks the cycle.
    Box::pin(async move {
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
                        // Re-run the full guard rather than forking directly:
                        // by the time the wait ends, another spawn/resume for
                        // this same address may already have won (this
                        // detached task has no ordering relative to the
                        // listener's own sequential processing, or to
                        // another such detached task), so the address could
                        // be live again, or even back to `completing` once
                        // more. `handle_spawn_request` re-checks state fresh
                        // and recurses through this same deferral if so,
                        // instead of blindly clobbering a run that won the
                        // race.
                        if let Err(e) = handle_spawn_request(&ctx, event).await {
                            tracing::warn!(error = %e, "failed to fork deferred session resume");
                        }
                    });
                    return Ok(());
                }
                SessionState::Forking | SessionState::Running | SessionState::Idle => {
                    // Two concurrent resume attempts for the same address
                    // (e.g. two messages queued while a session was
                    // completing, each deferred separately — see
                    // `AgentMessenger::deferred_resume`) can both observe the
                    // address as free and both end up publishing a spawn
                    // request. By the time this second one is processed, the
                    // first has already won and registered a live run —
                    // deliver this request's content into that run as a
                    // message instead of refusing it outright and losing it.
                    tracing::warn!(
                        address = %event.address,
                        state = %existing.state,
                        "spawn target is already live; delivering this request's input into it \
                         instead of forking a second run"
                    );
                    // Delivered as an agent message even for a `Conversation`
                    // trigger: this only fires in the narrow race window
                    // where two spawn/resume attempts for the same address
                    // land back to back, which is vanishingly rare for a
                    // single conversation's traffic. The message still
                    // reaches the session; it just doesn't carry the
                    // conversation's own sender attribution in that one case.
                    let content = event.kickoff_text();
                    let message = AgentMessageEvent {
                        from: event
                            .spawner
                            .clone()
                            .unwrap_or_else(|| SessionAddress::from(MAIN_ADDRESS)),
                        from_category: SessionCategory::from_trigger(&event.source)
                            .as_str()
                            .to_string(),
                        content,
                        hop_count: event.hop_count,
                    };
                    match ctx
                        .session_registry
                        .deliver(&event.address, Interrupt::AgentMessage(message))
                    {
                        DeliverOutcome::Delivered => {}
                        other @ (DeliverOutcome::Completing
                        | DeliverOutcome::Full
                        | DeliverOutcome::NotLive) => {
                            anyhow::bail!(
                                "session address {} is already live and delivering this \
                                 request's input into it failed ({other:?}); input dropped",
                                event.address
                            );
                        }
                    }
                    return Ok(());
                }
                SessionState::Completed => {}
            }
        }
        fork_and_spawn(ctx, event).await
    })
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
        event.address.clone(),
        event.depth,
        category,
        event.hop_count,
    )
    .await?;

    let request = SessionSpawnRequest {
        address: event.address,
        source_label: event.source_label,
        trigger: event.source,
        agent_skill: event.skill,
        spawner: event.spawner,
        depth: event.depth,
        subagent_config: SubAgentConfig {
            prompt: event.prompt,
            context: event.context,
            model_tier: event.model_tier,
            hop_count: event.hop_count,
            sender: event.sender,
        },
        conversation_target: event.conversation,
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
