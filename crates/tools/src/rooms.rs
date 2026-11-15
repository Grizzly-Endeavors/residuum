//! Participant-scoped `room_read` tool backed by [`residuum_rooms::RoomAgentView`].

use std::fmt::Write as _;

use async_trait::async_trait;
use chrono::SecondsFormat;
use serde_json::Value;

use residuum_agent::Tool;
use residuum_inference::{ToolDefinition, ToolError, ToolResult};
use residuum_rooms::{
    RoomAgentView, RoomMessageKind, RoomReadResult, RoomServiceError, RoomStatus, RoomSummary,
};

/// Default number of transcript messages returned when `limit` is omitted.
const DEFAULT_LIMIT: usize = 50;

/// Maximum number of transcript messages returned by `room_read`.
const MAX_LIMIT: usize = 200;

/// Built-in read-only tool allowing a participating agent to list its rooms
/// or read a room's metadata, minutes, and recent transcript messages.
pub struct RoomReadTool {
    view: RoomAgentView,
}

impl RoomReadTool {
    /// Construct a `room_read` tool scoped to `view`'s agent.
    #[must_use]
    pub fn new(view: RoomAgentView) -> Self {
        Self { view }
    }
}

fn parse_room_id(arguments: &Value) -> Result<Option<String>, ToolError> {
    match arguments.get("room_id") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "room_id must be a string, not {other}"
        ))),
    }
}

fn parse_limit(arguments: &Value) -> Result<usize, ToolError> {
    let Some(value) = arguments.get("limit").filter(|v| !v.is_null()) else {
        return Ok(DEFAULT_LIMIT);
    };
    let Some(number) = value.as_u64() else {
        return Err(ToolError::InvalidArguments(format!(
            "limit must be a non-negative integer, not {value}"
        )));
    };
    Ok(usize::try_from(number)
        .unwrap_or(MAX_LIMIT)
        .clamp(1, MAX_LIMIT))
}

const fn format_status(status: RoomStatus) -> &'static str {
    match status {
        RoomStatus::Active => "active",
        RoomStatus::Archived => "archived",
    }
}

const fn format_kind(kind: RoomMessageKind) -> &'static str {
    match kind {
        RoomMessageKind::Owner => "owner",
        RoomMessageKind::Agent => "agent",
        RoomMessageKind::System => "system",
    }
}

fn format_room_list(rooms: &[RoomSummary]) -> String {
    if rooms.is_empty() {
        return "No meeting rooms found.".to_owned();
    }

    let mut out = format!("{} room(s):", rooms.len());
    for room in rooms {
        let roster_names = if room.roster.is_empty() {
            "empty".to_owned()
        } else {
            room.roster
                .iter()
                .map(|seat| seat.display_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let updated = room.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true);
        _ = write!(
            out,
            "\n- \"{}\" (id: {}, status: {}, roster ({}): {}, messages: {}, updated: {}",
            room.name,
            room.id,
            format_status(room.status),
            room.roster.len(),
            roster_names,
            room.last_seq,
            updated,
        );
        if let Some(topic) = room.topic.as_deref().filter(|t| !t.trim().is_empty()) {
            _ = write!(out, ", topic: {topic}");
        }
        if let Some(minutes_path) = room
            .minutes_shared_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        {
            _ = write!(out, ", minutes: {minutes_path}");
        }
        out.push(')');
    }
    out
}

fn format_room_detail(result: &RoomReadResult) -> String {
    let mut out = String::new();
    _ = writeln!(out, "Room: {} (id: {})", result.room.name, result.room.id);
    _ = writeln!(out, "Status: {}", format_status(result.room.status));
    if let Some(topic) = result
        .room
        .topic
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        _ = writeln!(out, "Topic: {topic}");
    }
    let roster_text = if result.roster.is_empty() {
        "(empty)".to_owned()
    } else {
        result
            .roster
            .iter()
            .map(|seat| format!("{} ({})", seat.display_name, seat.seat_id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    _ = writeln!(out, "Roster ({}): {roster_text}", result.roster.len());
    _ = writeln!(
        out,
        "Messages: {} total (showing {})",
        result.room.last_seq,
        result.messages.len()
    );
    if let Some(shared_path) = result
        .room
        .minutes_shared_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        _ = writeln!(out, "Minutes path: {shared_path}");
    }

    if let Some(minutes) = result
        .minutes_markdown
        .as_deref()
        .filter(|m| !m.trim().is_empty())
    {
        _ = write!(out, "\n## Minutes\n{}", minutes.trim_end());
        out.push('\n');
    }

    _ = writeln!(
        out,
        "\n## Transcript (newest {} of {})",
        result.messages.len(),
        result.room.last_seq
    );
    if result.messages.is_empty() {
        out.push_str("(no messages yet)");
        return out;
    }

    for (idx, msg) in result.messages.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let ts = msg.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);
        _ = write!(
            out,
            "[{}] {} {} ({}): {}",
            msg.seq,
            ts,
            msg.author_name,
            format_kind(msg.kind),
            msg.content
        );
        if !msg.attachments.is_empty() {
            let names = msg
                .attachments
                .iter()
                .map(|att| att.filename.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            _ = write!(out, "\n  Attachments: {names}");
        }
    }
    out
}

#[async_trait]
impl Tool for RoomReadTool {
    fn name(&self) -> &'static str {
        "room_read"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "List meeting rooms you participate in (active or archived), or read a \
                          specific room's metadata, attached minutes, and recent transcript \
                          messages. Omit room_id to list your accessible rooms; pass room_id to \
                          read that room's details, minutes, and newest messages (up to limit, \
                          default 50, max 200)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "room_id": {
                        "type": "string",
                        "description": "Optional room ID (e.g. \"room_...\"). Omit to list all active and archived rooms you participate in."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of recent transcript messages to return when reading a room (default: 50, clamped to 1..=200)."
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let room_id = parse_room_id(&arguments)?;
        let limit = parse_limit(&arguments)?;

        let Some(room_id) = room_id else {
            let rooms = self.view.list_for_agent().await;
            return Ok(ToolResult::success(format_room_list(&rooms)));
        };

        match self.view.read_for_agent(&room_id, limit).await {
            Ok(detail) => Ok(ToolResult::success(format_room_detail(&detail))),
            Err(
                err @ (RoomServiceError::AccessDenied(_)
                | RoomServiceError::NotFound(_)
                | RoomServiceError::Validation(_)
                | RoomServiceError::BadRequest(_)),
            ) => Ok(ToolResult::error(err.to_string())),
            Err(
                err @ (RoomServiceError::Conflict(_)
                | RoomServiceError::Store(_)
                | RoomServiceError::Internal(_)),
            ) => Err(ToolError::Execution(err.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use residuum_a2a::A2aClientHub;
    use residuum_core::testing::wait;
    use residuum_inference::system_one::SystemOneService;
    use residuum_rooms::{
        CreateRoomRequest, LocalRoomAgentInfo, LocalRoomParticipant, LocalRoomTurnRequest,
        PatchRoomRequest, RoomAttachment, RoomEvent, RoomService, RoomStore, RoomTurnOutput,
    };
    use residuum_workspace::testing::{StaticMailboxDirectory, TeamHub};
    use tokio::sync::broadcast;

    use super::*;

    struct MockParticipant {
        name: String,
        replies: Mutex<VecDeque<Result<String, String>>>,
    }

    impl MockParticipant {
        fn new(name: &str, replies: Vec<Result<&str, &str>>) -> Arc<Self> {
            Arc::new(Self {
                name: name.to_owned(),
                replies: Mutex::new(
                    replies
                        .into_iter()
                        .map(|r| r.map(str::to_owned).map_err(str::to_owned))
                        .collect(),
                ),
            })
        }
    }

    #[async_trait]
    impl LocalRoomParticipant for MockParticipant {
        async fn run_room_turn(
            &self,
            _req: LocalRoomTurnRequest,
        ) -> Result<RoomTurnOutput, String> {
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(format!("Reply from {}", self.name)));
            reply.map(|content| RoomTurnOutput {
                content,
                attachments: Vec::new(),
            })
        }

        async fn cancel_room_turn(&self, _room_id: &str) -> Result<(), String> {
            Ok(())
        }

        async fn close_room_session(&self, _room_id: &str) -> Result<(), String> {
            Ok(())
        }

        async fn generate_minutes(&self, _prompt: String) -> Result<String, String> {
            Ok("# Meeting Minutes: Room Alpha\n\n## Summary\nReviewed architecture.\n\n## Decisions\n- Shipped room_read.\n\n## Action Items\n- Verify tests.\n".to_owned())
        }
    }

    async fn wait_until_idle(
        rx: &mut broadcast::Receiver<RoomEvent>,
        room_id: &str,
        expected_min_seq: u64,
        service: &RoomService,
    ) {
        wait::guarded("room turn loop to settle", async {
            loop {
                let detail = service.get_room(room_id, 1, None).await.unwrap();
                if detail.room.last_seq >= expected_min_seq
                    && detail.room.active_turn.is_none()
                    && detail.room.window.queued_seats.is_empty()
                {
                    return;
                }
                match rx.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
        .await;
    }

    async fn setup_rooms_fixture() -> (TeamHub, Arc<RoomService>, String, String) {
        let hub = TeamHub::new();
        let mailboxes = Arc::new(StaticMailboxDirectory::for_team_hub(
            &hub,
            &["scout", "helper", "other"],
        ));
        let store = RoomStore::open(hub.dir.path().join("hub").join("rooms"))
            .await
            .unwrap();
        let system_one = SystemOneService::new(None);
        let a2a_hub = A2aClientHub::new_shared();
        let service = RoomService::new(
            store,
            system_one,
            a2a_hub,
            hub.coordinator.clone(),
            mailboxes,
        )
        .await;

        let scout_part =
            MockParticipant::new("scout", vec![Ok("First scout note, @Helper check.")]);
        let helper_part = MockParticipant::new("helper", vec![Ok("Helper confirmed.")]);
        let other_part = MockParticipant::new("other", vec![Ok("Private room note.")]);

        service.register_local_agent(LocalRoomAgentInfo {
            name: "scout".to_owned(),
            display_name: "Scout".to_owned(),
            role: Some("Research".to_owned()),
            participant: scout_part,
        });
        service.register_local_agent(LocalRoomAgentInfo {
            name: "helper".to_owned(),
            display_name: "Helper".to_owned(),
            role: Some("Analysis".to_owned()),
            participant: helper_part,
        });
        service.register_local_agent(LocalRoomAgentInfo {
            name: "other".to_owned(),
            display_name: "Other".to_owned(),
            role: Some("Ops".to_owned()),
            participant: other_part,
        });

        let mut rx = service.subscribe_events();

        // Room A: scout + helper, 3 messages (owner + scout + helper) + archived minutes.
        let room_a = service
            .create_room(CreateRoomRequest {
                name: "Room Alpha".to_owned(),
                topic: Some("Design sync".to_owned()),
                seat_ids: vec!["agent:scout".to_owned(), "agent:helper".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let alpha_id = room_a.room.id.to_string();

        service
            .post_owner_message(
                &alpha_id,
                "@Scout please inspect.".to_owned(),
                vec![RoomAttachment {
                    id: "att_1".to_owned(),
                    filename: "spec.md".to_owned(),
                    mime_type: "text/markdown".to_owned(),
                    size: 128,
                    url: "/api/hub/uploads/att_1".to_owned(),
                }],
            )
            .await
            .unwrap();
        wait_until_idle(&mut rx, &alpha_id, 3, &service).await;

        let archived = service.archive_room(&alpha_id).await.unwrap();
        assert_eq!(
            archived.messages.len(),
            3,
            "Room A should contain owner + scout + helper messages"
        );
        assert!(
            archived.minutes_markdown.is_some(),
            "Room A should have generated minutes"
        );

        // Room B: other only.
        let room_b = service
            .create_room(CreateRoomRequest {
                name: "Room Beta".to_owned(),
                topic: Some("Private ops".to_owned()),
                seat_ids: vec!["agent:other".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let beta_id = room_b.room.id.to_string();

        (hub, service, alpha_id, beta_id)
    }

    #[tokio::test]
    async fn list_mode_shows_only_participating_rooms() {
        let (_hub, service, alpha_id, beta_id) = setup_rooms_fixture().await;
        let tool = RoomReadTool::new(service.agent_view("scout"));

        let res = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!res.is_error, "list mode should succeed: {}", res.output);
        assert!(
            res.output.contains(&alpha_id),
            "scout should see Room A ({alpha_id}): {}",
            res.output
        );
        assert!(
            res.output.contains("Room Alpha"),
            "output should include room title: {}",
            res.output
        );
        assert!(
            res.output.contains("archived"),
            "output should include room status: {}",
            res.output
        );
        assert!(
            res.output.contains("Design sync"),
            "output should include room topic: {}",
            res.output
        );
        assert!(
            res.output.contains("roster (2): Scout, Helper"),
            "output should include roster count and names: {}",
            res.output
        );
        assert!(
            res.output.contains("messages: 3"),
            "output should include last_seq message count: {}",
            res.output
        );
        assert!(
            res.output.contains("team/minutes/"),
            "output should include minutes shared_path: {}",
            res.output
        );
        assert!(
            !res.output.contains(&beta_id) && !res.output.contains("Room Beta"),
            "scout must NOT see Room B: {}",
            res.output
        );

        // Empty or null room_id also invokes list mode.
        let res_empty = tool
            .execute(serde_json::json!({"room_id": "   "}))
            .await
            .unwrap();
        assert!(!res_empty.is_error, "blank room_id should list");
        assert!(
            res_empty.output.contains(&alpha_id),
            "blank room_id should return Room A"
        );

        let res_null = tool
            .execute(serde_json::json!({"room_id": null}))
            .await
            .unwrap();
        assert!(!res_null.is_error, "null room_id should list");
        assert!(
            res_null.output.contains(&alpha_id),
            "null room_id should return Room A"
        );
    }

    #[tokio::test]
    async fn read_mode_returns_metadata_minutes_and_newest_limited_messages() {
        let (_hub, service, alpha_id, _beta_id) = setup_rooms_fixture().await;
        let tool = RoomReadTool::new(service.agent_view("scout"));

        let res = tool
            .execute(serde_json::json!({"room_id": alpha_id, "limit": 2}))
            .await
            .unwrap();
        assert!(!res.is_error, "read mode should succeed: {}", res.output);
        assert!(
            res.output.contains("Room Alpha"),
            "header should contain room title: {}",
            res.output
        );
        assert!(
            res.output.contains("Design sync"),
            "header should contain topic: {}",
            res.output
        );
        assert!(
            res.output.contains("Shipped room_read."),
            "output should include attached minutes: {}",
            res.output
        );
        assert!(
            res.output.contains("team/minutes/"),
            "output should include minutes shared_path: {}",
            res.output
        );
        // Should contain newest 2 messages ([2] and [3]), NOT message [1].
        assert!(
            !res.output.contains("[1] "),
            "limit=2 should omit oldest message [1]: {}",
            res.output
        );
        assert!(
            res.output.contains("[2] ") && res.output.contains("First scout note"),
            "limit=2 should include message [2]: {}",
            res.output
        );
        assert!(
            res.output.contains("[3] ") && res.output.contains("Helper confirmed."),
            "limit=2 should include message [3]: {}",
            res.output
        );

        // Default limit (omitted) shows all 3 including attachment filename on message [1].
        let full = tool
            .execute(serde_json::json!({"room_id": alpha_id}))
            .await
            .unwrap();
        assert!(!full.is_error, "default read should succeed");
        assert!(
            full.output.contains("[1] ") && full.output.contains("Attachments: spec.md"),
            "message [1] should format attachment filename: {}",
            full.output
        );
    }

    #[tokio::test]
    async fn non_participant_room_read_returns_permission_error() {
        let (_hub, service, _alpha_id, beta_id) = setup_rooms_fixture().await;
        let tool = RoomReadTool::new(service.agent_view("scout"));

        let res = tool
            .execute(serde_json::json!({"room_id": beta_id}))
            .await
            .unwrap();
        assert!(res.is_error, "reading non-participant room must error");
        assert!(
            res.output.contains("not a participant"),
            "error should explain non-participant denial: {}",
            res.output
        );
    }

    #[tokio::test]
    async fn removed_historical_participant_still_lists_and_reads_room() {
        let (_hub, service, alpha_id, _beta_id) = setup_rooms_fixture().await;

        // Reopen Room A and remove `agent:scout` from the active roster.
        service.reopen_room(&alpha_id).await.unwrap();
        service
            .patch_room(
                &alpha_id,
                PatchRoomRequest {
                    remove_seats: Some(vec!["agent:scout".to_owned()]),
                    ..PatchRoomRequest::default()
                },
            )
            .await
            .unwrap();

        let tool = RoomReadTool::new(service.agent_view("scout"));
        let list_res = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            !list_res.is_error && list_res.output.contains(&alpha_id),
            "historical participant should still see Room A in list mode: {}",
            list_res.output
        );

        let read_res = tool
            .execute(serde_json::json!({"room_id": alpha_id}))
            .await
            .unwrap();
        assert!(
            !read_res.is_error && read_res.output.contains("Helper confirmed."),
            "historical participant should still read Room A per FR-1.4: {}",
            read_res.output
        );
    }

    #[tokio::test]
    async fn list_mode_empty_and_invalid_arguments() {
        let (_hub, service, alpha_id, _beta_id) = setup_rooms_fixture().await;
        let outsider_tool = RoomReadTool::new(service.agent_view("outsider"));

        let empty_list = outsider_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!empty_list.is_error, "empty list should succeed");
        assert!(
            empty_list.output.contains("No meeting rooms"),
            "empty list message: {}",
            empty_list.output
        );

        let scout_tool = RoomReadTool::new(service.agent_view("scout"));
        for bad_limit in [
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("5"),
            serde_json::json!(true),
        ] {
            let err = scout_tool
                .execute(serde_json::json!({"room_id": alpha_id, "limit": bad_limit}))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ToolError::InvalidArguments(_)),
                "expected InvalidArguments for bad limit, got {err:?}"
            );
        }

        let bad_room_id = scout_tool
            .execute(serde_json::json!({"room_id": 42}))
            .await
            .unwrap_err();
        assert!(
            matches!(bad_room_id, ToolError::InvalidArguments(_)),
            "expected InvalidArguments for non-string room_id, got {bad_room_id:?}"
        );

        // Invalid room ID format and nonexistent room ID both return clear tool errors.
        let invalid_id = scout_tool
            .execute(serde_json::json!({"room_id": "../secret"}))
            .await
            .unwrap();
        assert!(invalid_id.is_error, "invalid room_id should return error");
        assert!(
            invalid_id.output.contains("invalid room id"),
            "invalid room_id error: {}",
            invalid_id.output
        );

        let missing_id = scout_tool
            .execute(serde_json::json!({"room_id": "room_00000000000000"}))
            .await
            .unwrap();
        assert!(missing_id.is_error, "missing room_id should return error");
        assert!(
            missing_id.output.contains("not found"),
            "missing room_id error: {}",
            missing_id.output
        );

        // limit: 0 clamps to 1 and limit: 500 clamps to 200.
        let clamped_low = scout_tool
            .execute(serde_json::json!({"room_id": alpha_id, "limit": 0}))
            .await
            .unwrap();
        assert!(!clamped_low.is_error, "limit 0 should clamp to 1");
        assert!(
            clamped_low.output.contains("[3] ") && !clamped_low.output.contains("[2] "),
            "clamped limit=1 should show only message [3]: {}",
            clamped_low.output
        );

        let clamped_high = scout_tool
            .execute(serde_json::json!({"room_id": alpha_id, "limit": 500}))
            .await
            .unwrap();
        assert!(!clamped_high.is_error, "limit 500 should clamp to 200");
        assert!(
            clamped_high.output.contains("[1] ") && clamped_high.output.contains("[3] "),
            "clamped limit=200 should show all 3 messages: {}",
            clamped_high.output
        );
    }

    #[tokio::test]
    async fn system_notice_kind_is_formatted_in_transcript() {
        let hub = TeamHub::new();
        let mailboxes = Arc::new(StaticMailboxDirectory::for_team_hub(&hub, &["scout"]));
        let store = RoomStore::open(hub.dir.path().join("hub").join("rooms"))
            .await
            .unwrap();
        let service = RoomService::new(
            store,
            SystemOneService::new(None),
            A2aClientHub::new_shared(),
            hub.coordinator.clone(),
            mailboxes,
        )
        .await;

        service.register_local_agent(LocalRoomAgentInfo {
            name: "scout".to_owned(),
            display_name: "Scout".to_owned(),
            role: None,
            participant: MockParticipant::new("scout", vec![Err("offline")]),
        });

        let created = service
            .create_room(CreateRoomRequest {
                name: "System Test".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();
        service
            .post_owner_message(&room_id, "@Scout go".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, 2, &service).await;

        let tool = RoomReadTool::new(service.agent_view("scout"));
        let res = tool
            .execute(serde_json::json!({"room_id": room_id}))
            .await
            .unwrap();
        assert!(!res.is_error, "read should succeed: {}", res.output);
        assert!(
            res.output.contains("(system):"),
            "system notice should show (system) kind: {}",
            res.output
        );
    }
}
