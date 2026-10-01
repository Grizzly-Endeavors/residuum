//! The session relay of the hub WebSocket.
//!
//! A connection subscribes to one session of one agent, or to every session
//! an artifact started on any agent. The hub watches each running agent's
//! sessions (see [`crate::hub::agent_watch`]) and sends the connection every
//! event of the sessions it follows as a `session_frame`: the `session_*`
//! frame the agent's own socket sends, tool frames included, because the hub
//! socket has no verbose flag.
//!
//! Subscriptions belong to the connection and end with it. A connection reads
//! the relay from the moment it opens and filters by what it subscribed to,
//! so a subscription to an artifact also follows the sessions that start
//! after it, on any agent, with no further message.

use std::collections::{HashMap, HashSet};

use tokio::sync::broadcast::{self, error::RecvError};

use super::ws::{Outbound, notice_frame, send_frame};
use crate::background::registry::ARTIFACT_SENDER_PREFIX;
use crate::gateway::sessions::session_event_to_server_message;
use crate::hub::AgentDirectory;
use crate::hub::agent_watch::{AgentChangeFeed, AgentSessionEvent};
use crate::hub::types::{HubSocketFrame, SessionSubscriptionKind};

/// What one hub socket connection follows, and its end of the relay.
pub(super) struct SessionRelay {
    feed: broadcast::Receiver<AgentSessionEvent>,
    /// The addresses followed on each agent.
    sessions: HashMap<String, HashSet<String>>,
    /// The artifacts whose sessions are followed.
    artifacts: HashSet<String>,
}

impl SessionRelay {
    /// A connection's relay, following nothing yet. It hears every session
    /// event from now on, so a subscription made later misses none that
    /// follow it.
    pub(super) fn new(changes: &AgentChangeFeed) -> Self {
        Self {
            feed: changes.subscribe_sessions(),
            sessions: HashMap::new(),
            artifacts: HashSet::new(),
        }
    }

    fn follows_anything(&self) -> bool {
        !(self.sessions.is_empty() && self.artifacts.is_empty())
    }

    fn follows(&self, event: &AgentSessionEvent) -> bool {
        let by_address = self
            .sessions
            .get(&event.agent)
            .is_some_and(|addresses| addresses.contains(event.event.address.as_ref()));
        by_address
            || event
                .source_label
                .as_deref()
                .and_then(|label| label.strip_prefix(ARTIFACT_SENDER_PREFIX))
                .is_some_and(|artifact| self.artifacts.contains(artifact))
    }

    /// The next relayed event this connection follows, or the loss of
    /// events. Events of sessions it doesn't follow are skipped.
    pub(super) async fn next(&mut self) -> Result<AgentSessionEvent, RecvError> {
        loop {
            let event = self.feed.recv().await?;
            if self.follows(&event) {
                return Ok(event);
            }
        }
    }

    /// Send what [`Self::next`] returned. `false` when the connection should
    /// end.
    pub(super) async fn forward(
        &self,
        outbound: &mut Outbound,
        received: Result<AgentSessionEvent, RecvError>,
    ) -> bool {
        match received {
            Ok(AgentSessionEvent { agent, event, .. }) => {
                let frame = HubSocketFrame::SessionFrame {
                    agent,
                    frame: session_event_to_server_message(event),
                };
                send_frame(outbound, &frame).await
            }
            Err(RecvError::Lagged(missed)) => {
                // A connection that follows nothing has lost nothing.
                if !self.follows_anything() {
                    return true;
                }
                tracing::warn!(
                    missed,
                    "hub websocket fell behind the session relay; telling the client to read its sessions again"
                );
                send_frame(outbound, &HubSocketFrame::SessionRelayLagged).await
            }
            Err(RecvError::Closed) => false,
        }
    }

    /// Follow one session. An unknown agent is refused with a notice and
    /// no acknowledgement. `false` when the connection should end.
    pub(super) async fn subscribe_session(
        &mut self,
        outbound: &mut Outbound,
        directory: &dyn AgentDirectory,
        agent: String,
        address: String,
    ) -> bool {
        if directory.summary(&agent).is_err() {
            return refuse_unknown_agent(outbound, &agent).await;
        }
        self.sessions
            .entry(agent.clone())
            .or_default()
            .insert(address.clone());
        let ack = HubSocketFrame::Subscribed {
            kind: SessionSubscriptionKind::Session,
            agent: Some(agent),
            address: Some(address),
            artifact: None,
        };
        send_frame(outbound, &ack).await
    }

    /// Stop following a session. Stopping one that isn't followed does
    /// nothing.
    pub(super) fn unsubscribe_session(&mut self, agent: &str, address: &str) {
        if let Some(addresses) = self.sessions.get_mut(agent) {
            addresses.remove(address);
            if addresses.is_empty() {
                self.sessions.remove(agent);
            }
        }
    }

    /// Follow every session of an artifact, on any agent, now and later.
    /// `false` when the connection should end.
    pub(super) async fn subscribe_artifact_sessions(
        &mut self,
        outbound: &mut Outbound,
        artifact: String,
    ) -> bool {
        self.artifacts.insert(artifact.clone());
        let ack = HubSocketFrame::Subscribed {
            kind: SessionSubscriptionKind::ArtifactSessions,
            agent: None,
            address: None,
            artifact: Some(artifact),
        };
        send_frame(outbound, &ack).await
    }

    /// Stop following an artifact's sessions. Stopping one that isn't
    /// followed does nothing.
    pub(super) fn unsubscribe_artifact_sessions(&mut self, artifact: &str) {
        self.artifacts.remove(artifact);
    }
}

async fn refuse_unknown_agent(outbound: &mut Outbound, agent: &str) -> bool {
    tracing::warn!(agent, "refused a session subscription for an unknown agent");
    send_frame(
        outbound,
        &notice_frame(
            "warn",
            &format!("Couldn't follow sessions on {agent:?}: no agent has that name."),
        ),
    )
    .await
}
