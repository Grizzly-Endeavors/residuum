//! Per-agent room view: implements [`RoomRoster`] for `<ROOMS>` system-prompt
//! injection and participant-scoped lookup helpers for the `room_read` tool.

use std::fmt::Write as _;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use residuum_agent::RoomRoster;

use crate::service::{RoomService, RoomServiceError};
use crate::types::{RoomMessage, RoomRecord, RoomSeat, RoomStatus, RoomSummary};

/// Transcript and minutes slice returned to a participating agent by `room_read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomReadResult {
    /// Room summary header.
    pub room: RoomSummary,
    /// Active roster seats.
    pub roster: Vec<RoomSeat>,
    /// Attached meeting minutes markdown, if generated.
    pub minutes_markdown: Option<String>,
    /// Recent transcript messages, oldest-to-newest.
    pub messages: Vec<RoomMessage>,
}

/// Agent-scoped handle over [`RoomService`].
#[derive(Clone)]
pub struct RoomAgentView {
    agent_name: String,
    seat_id: String,
    service: Arc<RoomService>,
}

impl RoomAgentView {
    pub(crate) fn new(agent_name: impl Into<String>, service: Arc<RoomService>) -> Self {
        let agent_name = agent_name.into();
        let seat_id = format!("agent:{agent_name}");
        Self {
            agent_name,
            seat_id,
            service,
        }
    }

    /// List all active and archived rooms this agent participates in (or
    /// previously participated in), sorted by `updated_at` descending.
    pub async fn list_for_agent(&self) -> Vec<RoomSummary> {
        self.service
            .store()
            .list_records()
            .await
            .iter()
            .filter(|rec| rec.is_participant(&self.seat_id))
            .map(RoomSummary::from_record)
            .collect()
    }

    /// Read a room's summary, minutes, and recent transcript if this agent is
    /// on its roster or `historical_participants`.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::AccessDenied`] if the agent is not a
    /// participant, or [`RoomServiceError::NotFound`] / validation / store
    /// errors.
    pub async fn read_for_agent(
        &self,
        room_id: &str,
        limit: usize,
    ) -> Result<RoomReadResult, RoomServiceError> {
        let record = self.service.store().get_record(room_id).await?;
        if !record.is_participant(&self.seat_id) {
            return Err(RoomServiceError::AccessDenied(format!(
                "agent '{}' is not a participant in room '{room_id}'",
                self.agent_name
            )));
        }
        let clamped = limit.clamp(1, 200);
        let messages = self
            .service
            .store()
            .read_messages(room_id, clamped, None)
            .await?;
        let minutes_markdown = self.service.store().read_minutes(room_id).await?;
        Ok(RoomReadResult {
            room: RoomSummary::from_record(&record),
            roster: record.roster,
            minutes_markdown,
            messages,
        })
    }
}

impl RoomRoster for RoomAgentView {
    fn prompt_block(&self) -> Option<String> {
        let mut rooms: Vec<RoomRecord> = self
            .service
            .snapshot_records()
            .into_iter()
            .filter(|rec| rec.is_participant(&self.seat_id))
            .collect();
        if rooms.is_empty() {
            return None;
        }
        rooms.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });

        let now = Utc::now();
        let mut block =
            "Rooms you participate in (use room_read to inspect transcripts or minutes):"
                .to_owned();
        for room in &rooms {
            match room.status {
                RoomStatus::Active => {
                    let count = room.roster.len();
                    let noun = if count == 1 {
                        "participant"
                    } else {
                        "participants"
                    };
                    let recency = if room.last_seq == 0 {
                        "no messages yet".to_owned()
                    } else {
                        format!("last message {}", format_relative_age(now, room.updated_at))
                    };
                    _ = write!(
                        block,
                        "\n- \"{}\" (id: {}, active, {count} {noun}, {recency})",
                        room.name, room.id
                    );
                }
                RoomStatus::Archived => {
                    if let Some(shared_path) = &room.minutes.shared_path {
                        _ = write!(
                            block,
                            "\n- \"{}\" (id: {}, archived, minutes at {shared_path})",
                            room.name, room.id
                        );
                    } else {
                        let count = room.roster.len();
                        let noun = if count == 1 {
                            "participant"
                        } else {
                            "participants"
                        };
                        _ = write!(
                            block,
                            "\n- \"{}\" (id: {}, archived, {count} {noun})",
                            room.name, room.id
                        );
                    }
                }
            }
        }
        Some(block)
    }
}

fn format_relative_age(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds().max(0);
    if secs < 60 {
        "just now".to_owned()
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}
