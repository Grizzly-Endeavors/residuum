//! The hub WebSocket, `/api/hub/ws`.
//!
//! Server to client: `hub_boot`, the `system_one_status` (sent again whenever
//! that status changes), and an `agents_snapshot` on connect, then
//! every hub event (`agent_state`, `agent_stopping`, `agent_created`,
//! `agent_restored`, `agent_deleted`, `agent_activity`, `notice`,
//! `hub_config_reloaded`), a `team_event` for every entry the team event log
//! records, an `agent_overview` whenever something in an agent's overview
//! changes, `artifact_updated` and `artifact_removed` for the workbench (see
//! [`super::artifact_events`]), and `workspace_changed` frames for the team
//! paths the client watches. Client to server: `watch_team`, `presence`
//! reports for Web Push, `ping` (answered with `pong` once every earlier
//! message has been handled), and the session subscriptions of
//! [`super::session_relay`], which answer `subscribed` and deliver
//! `session_frame`s.
//!
//! A connection that falls behind the hub's event stream, the team event
//! log or the overview frames can't know what it missed, so it gets a fresh
//! `agents_snapshot` in place of the lost events, and the client reads the
//! log and the overview again. One that falls behind the session relay gets
//! `session_relay_lagged`, and the client reads its sessions again.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::json;
use tokio::sync::{broadcast, watch};

use super::artifact_events::ArtifactEvents;
use super::session_relay::SessionRelay;
use crate::bus::{BusHandle, Subscriber, WorkspaceEvent, topics};
use crate::gateway::protocol::ServerMessage;
use crate::hub::agent_watch::AgentChangeFeed;
use crate::hub::overview::{AgentOverview, TeamOverview};
use crate::hub::push::{Presence, PresenceConnection};
use crate::hub::team_events::{TeamEvent, TeamEventLog};
use crate::hub::types::{AgentListResponse, HubClientMessage, HubSocketFrame};
use crate::hub::{AgentDirectory, HubEvent};
use crate::inference::system_one::{SystemOneService, SystemOneStatus};
use crate::interfaces::websocket::subscriber::workspace_frame;
use crate::workspace::watch::{LIVE_UPDATES_OFF_MESSAGE, WatchHealth, WatchSet};

/// The namespace prefix of team paths in the change feed.
const TEAM_PREFIX: &str = crate::workspace::team_files::TEAM_PREFIX;

/// What a hub WebSocket connection reads.
#[derive(Clone)]
pub(super) struct HubWsState {
    pub directory: Arc<dyn AgentDirectory>,
    pub team_bus: BusHandle,
    pub team_watch_health: watch::Receiver<WatchHealth>,
    pub team_events: Arc<TeamEventLog>,
    pub overview: Arc<TeamOverview>,
    pub agent_changes: Arc<AgentChangeFeed>,
    /// Where clients report whether their push device is in front of the user.
    pub presence: Arc<Presence>,
    /// The System 1 service. Each connection subscribes to its status, sends
    /// the current value after `hub_boot`, and every change after.
    pub system_one: Arc<SystemOneService>,
}

/// The route that upgrades to the hub WebSocket.
pub(super) fn routes(state: HubWsState) -> Router {
    Router::new()
        .route("/api/hub/ws", get(upgrade))
        .with_state(state)
}

async fn upgrade(ws: WebSocketUpgrade, State(state): State<HubWsState>) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state))
}

pub(super) type Outbound = SplitSink<WebSocket, Message>;

/// Serve one connection until the client leaves or the hub's event stream
/// closes.
async fn serve(socket: WebSocket, state: HubWsState) {
    let (mut outbound, mut inbound) = socket.split();

    // Subscribe before reading the list, so a change between the two reaches
    // the client as an event instead of falling in the gap.
    let mut events = state.directory.subscribe();
    let mut log_entries = state.team_events.subscribe();
    let mut overviews = state.overview.subscribe();
    let mut team_feed = subscribe_team_feed(&state.team_bus).await;
    let mut artifacts = ArtifactEvents::subscribe(&state.team_bus).await;
    let mut sessions = SessionRelay::new(&state.agent_changes);
    let mut watch_set = WatchSet::default();
    // What this connection reports about its push device ends when it closes.
    let presence = state.presence.connect();

    let boot = HubSocketFrame::HubBoot {
        boot_id: state.team_events.boot_id().to_string(),
    };
    if !send_frame(&mut outbound, &boot).await {
        return;
    }
    let mut system_one = state.system_one.subscribe();
    if !send_system_one_status(&mut outbound, &mut system_one).await {
        return;
    }
    if !send_snapshot(&mut outbound, state.directory.as_ref()).await {
        return;
    }
    if team_feed.is_none() && !send_frame(&mut outbound, &unavailable_frame()).await {
        return;
    }

    loop {
        let alive = tokio::select! {
            Ok(()) = system_one.changed() => {
                send_system_one_status(&mut outbound, &mut system_one).await
            }
            event = events.recv() => {
                forward_hub_event(&mut outbound, state.directory.as_ref(), event).await
            }
            entry = log_entries.recv() => {
                forward_log_entry(&mut outbound, &state, entry).await
            }
            overview = overviews.recv() => {
                forward_overview(&mut outbound, state.directory.as_ref(), overview).await
            }
            event = next_team_event(&mut team_feed) => {
                forward_team_event(&mut outbound, &watch_set, &mut team_feed, event).await
            }
            event = artifacts.next() => {
                artifacts.forward(&mut outbound, event).await
            }
            relayed = sessions.next() => {
                sessions.forward(&mut outbound, relayed).await
            }
            frame = inbound.next() => {
                handle_client_frame(
                    &mut outbound,
                    &state,
                    &mut watch_set,
                    &mut sessions,
                    &presence,
                    frame,
                )
                .await
            }
        };
        if !alive {
            return;
        }
    }
}

async fn subscribe_team_feed(bus: &BusHandle) -> Option<Subscriber<WorkspaceEvent>> {
    match bus.subscribe(topics::Workspace).await {
        Ok(subscriber) => Some(subscriber),
        Err(e) => {
            tracing::warn!(error = %e, "hub websocket couldn't subscribe to the team change feed; team files won't update live");
            None
        }
    }
}

/// The next event of the team change feed, or never when there is no feed.
async fn next_team_event(
    feed: &mut Option<Subscriber<WorkspaceEvent>>,
) -> Result<Option<WorkspaceEvent>, crate::bus::BusError> {
    match feed {
        Some(feed) => feed.recv().await,
        None => std::future::pending().await,
    }
}

/// Send `frame` as a JSON text message. `false` when the client is gone.
pub(super) async fn send_frame(outbound: &mut Outbound, frame: &impl Serialize) -> bool {
    let text = match serde_json::to_string(frame) {
        Ok(text) => text,
        Err(e) => {
            tracing::error!(error = %e, "failed to serialize a hub websocket frame");
            return true;
        }
    };
    outbound.send(Message::text(text)).await.is_ok()
}

async fn send_system_one_status(
    outbound: &mut Outbound,
    status: &mut watch::Receiver<SystemOneStatus>,
) -> bool {
    let frame = HubSocketFrame::SystemOneStatus {
        status: status.borrow_and_update().clone(),
    };
    send_frame(outbound, &frame).await
}

async fn send_snapshot(outbound: &mut Outbound, directory: &dyn AgentDirectory) -> bool {
    let snapshot = HubSocketFrame::AgentsSnapshot(AgentListResponse::of(directory));
    send_frame(outbound, &snapshot).await
}

pub(super) fn notice_frame(level: &str, message: &str) -> serde_json::Value {
    json!({ "type": "notice", "level": level, "message": message })
}

fn unavailable_frame() -> ServerMessage {
    ServerMessage::WorkspaceWatchUnavailable {
        message: LIVE_UPDATES_OFF_MESSAGE.to_string(),
    }
}

/// Forward one hub event, or a fresh snapshot when events were lost. `false`
/// when the connection should end.
async fn forward_hub_event(
    outbound: &mut Outbound,
    directory: &dyn AgentDirectory,
    event: Result<HubEvent, broadcast::error::RecvError>,
) -> bool {
    match event {
        Ok(event) => send_frame(outbound, &event).await,
        Err(broadcast::error::RecvError::Lagged(missed)) => {
            tracing::warn!(
                missed,
                "hub websocket fell behind; resending the agent snapshot"
            );
            send_snapshot(outbound, directory).await
        }
        Err(broadcast::error::RecvError::Closed) => false,
    }
}

/// Forward one new team event log entry, or a fresh snapshot when entries
/// were lost. `false` when the connection should end.
async fn forward_log_entry(
    outbound: &mut Outbound,
    state: &HubWsState,
    entry: Result<TeamEvent, broadcast::error::RecvError>,
) -> bool {
    match entry {
        Ok(event) => {
            let frame = HubSocketFrame::TeamEvent {
                boot_id: state.team_events.boot_id().to_string(),
                event,
            };
            send_frame(outbound, &frame).await
        }
        Err(broadcast::error::RecvError::Lagged(missed)) => {
            tracing::warn!(
                missed,
                "hub websocket fell behind the team event log; resending the agent snapshot"
            );
            send_snapshot(outbound, state.directory.as_ref()).await
        }
        Err(broadcast::error::RecvError::Closed) => false,
    }
}

/// Forward one agent's changed overview, or a fresh snapshot when overviews
/// were lost. `false` when the connection should end.
async fn forward_overview(
    outbound: &mut Outbound,
    directory: &dyn AgentDirectory,
    overview: Result<AgentOverview, broadcast::error::RecvError>,
) -> bool {
    match overview {
        Ok(overview) => send_frame(outbound, &HubSocketFrame::AgentOverview { overview }).await,
        Err(broadcast::error::RecvError::Lagged(missed)) => {
            tracing::warn!(
                missed,
                "hub websocket fell behind the team overview; resending the agent snapshot"
            );
            send_snapshot(outbound, directory).await
        }
        Err(broadcast::error::RecvError::Closed) => false,
    }
}

/// Forward one team change-feed event to a client watching `watch_set`.
async fn forward_team_event(
    outbound: &mut Outbound,
    watch_set: &WatchSet,
    feed: &mut Option<Subscriber<WorkspaceEvent>>,
    event: Result<Option<WorkspaceEvent>, crate::bus::BusError>,
) -> bool {
    if let Ok(Some(event)) = event {
        return match workspace_frame(watch_set, event) {
            Some(frame) => send_frame(outbound, &frame).await,
            None => true,
        };
    }
    tracing::warn!("the team change feed ended; the hub websocket stops forwarding team files");
    *feed = None;
    watch_set.is_empty() || send_frame(outbound, &unavailable_frame()).await
}

/// Act on one frame from the client. `false` when the connection should end.
async fn handle_client_frame(
    outbound: &mut Outbound,
    state: &HubWsState,
    watch_set: &mut WatchSet,
    sessions: &mut SessionRelay,
    presence: &PresenceConnection,
    frame: Option<Result<Message, axum::Error>>,
) -> bool {
    let text = match frame {
        Some(Ok(Message::Text(text))) => text,
        Some(Ok(Message::Close(frame))) => {
            // Axum doesn't answer a client's Close frame on its own: without
            // an explicit reply, and a shutdown of the transport to go with
            // it, the client sees code 1006 (abnormal closure) instead of
            // the code it asked to close with.
            outbound.send(Message::Close(frame)).await.ok();
            outbound.close().await.ok();
            return false;
        }
        Some(Ok(_)) => return true,
        Some(Err(_)) | None => return false,
    };
    let request: HubClientMessage = match serde_json::from_str(&text) {
        Ok(request) => request,
        Err(e) => {
            tracing::warn!(error = %e, "malformed hub websocket message from a client");
            return send_frame(
                outbound,
                &notice_frame(
                    "warn",
                    "Residuum couldn't read a message from this page. Reload the page if team files or sessions stop updating.",
                ),
            )
            .await;
        }
    };
    let directory = state.directory.as_ref();
    match request {
        HubClientMessage::WatchTeam { prefixes } => {
            watch_team(outbound, state, watch_set, prefixes).await
        }
        HubClientMessage::Presence { device_id, active } => {
            presence.report(&device_id, active);
            true
        }
        HubClientMessage::SubscribeSession { agent, address } => {
            sessions
                .subscribe_session(outbound, directory, agent, address)
                .await
        }
        HubClientMessage::UnsubscribeSession { agent, address } => {
            sessions.unsubscribe_session(&agent, &address);
            true
        }
        HubClientMessage::SubscribeArtifactSessions { artifact } => {
            sessions
                .subscribe_artifact_sessions(outbound, artifact)
                .await
        }
        HubClientMessage::UnsubscribeArtifactSessions { artifact } => {
            sessions.unsubscribe_artifact_sessions(&artifact);
            true
        }
        HubClientMessage::Ping => send_frame(outbound, &HubSocketFrame::Pong).await,
    }
}

/// Replace the team paths the connection watches. `false` when the
/// connection should end.
async fn watch_team(
    outbound: &mut Outbound,
    state: &HubWsState,
    watch_set: &mut WatchSet,
    prefixes: Vec<String>,
) -> bool {
    match parse_team_prefixes(prefixes) {
        Ok(set) => {
            let watching = !set.is_empty();
            *watch_set = set;
            if watching && *state.team_watch_health.borrow() == WatchHealth::Off {
                return send_frame(outbound, &unavailable_frame()).await;
            }
            true
        }
        Err(message) => {
            tracing::warn!(reason = %message, "refused a team watch request");
            send_frame(outbound, &notice_frame("warn", &message)).await
        }
    }
}

/// Validate a `watch_team` request: every prefix names `team` or a path
/// under `team/`, which is how the change feed spells team paths.
fn parse_team_prefixes(prefixes: Vec<String>) -> Result<WatchSet, String> {
    for prefix in &prefixes {
        let in_team = prefix
            .strip_prefix(TEAM_PREFIX)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
        if !in_team {
            return Err(format!(
                "Couldn't watch {prefix:?}: team watch paths start with {TEAM_PREFIX}/, like \"{TEAM_PREFIX}/wiki\"."
            ));
        }
    }
    WatchSet::parse(prefixes).map_err(|e| format!("Couldn't watch the team files: {e}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_prefixes_must_live_under_team() {
        assert!(parse_team_prefixes(vec!["team/wiki".into(), "team".into()]).is_ok());
        assert!(parse_team_prefixes(vec![]).is_ok());
        for bad in ["wiki", "", "teamwork/wiki", "/team/wiki"] {
            assert!(
                parse_team_prefixes(vec![bad.to_string()]).is_err(),
                "{bad:?} should be refused"
            );
        }
        assert!(parse_team_prefixes(vec!["team/../memory".into()]).is_err());
    }
}
