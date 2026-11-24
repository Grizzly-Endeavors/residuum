use std::collections::{HashMap, HashSet};

use tokio::sync::broadcast::{self, error::RecvError};

use super::ws::{Outbound, notice_frame, send_frame};
use crate::AgentDirectory;
use crate::agent_watch::{AgentChangeFeed, AgentSessionEvent};
use crate::types::{HubSocketFrame, SessionSubscriptionKind};
use residuum_bus::session::ARTIFACT_SENDER_PREFIX;
use residuum_protocol::frames::session_event_to_server_message;

pub(super) struct SessionRelay {
    feed: broadcast::Receiver<AgentSessionEvent>,
    sessions: HashMap<String, HashSet<String>>,
    artifacts: HashSet<String>,
}

impl SessionRelay {
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

    pub(super) async fn next(&mut self) -> Result<AgentSessionEvent, RecvError> {
        loop {
            let event = self.feed.recv().await?;
            if self.follows(&event) {
                return Ok(event);
            }
        }
    }

    pub(super) async fn forward(
        &self,
        outbound: &mut Outbound,
        received: Result<AgentSessionEvent, RecvError>,
    ) -> bool {
        match received {
            Ok(AgentSessionEvent { agent, event, .. }) => {
                let frame = HubSocketFrame::SessionFrame {
                    agent,
                    frame: Box::new(session_event_to_server_message(event)),
                };
                send_frame(outbound, &frame).await
            }
            Err(RecvError::Lagged(missed)) => {
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

    pub(super) fn unsubscribe_session(&mut self, agent: &str, address: &str) {
        if let Some(addresses) = self.sessions.get_mut(agent) {
            addresses.remove(address);
            if addresses.is_empty() {
                self.sessions.remove(agent);
            }
        }
    }

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
