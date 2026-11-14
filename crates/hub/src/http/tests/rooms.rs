//! Integration tests for `/api/hub/rooms/*` and room frames on `/api/hub/ws`.

use super::*;
use residuum_core::testing::gate::Gate;
use residuum_rooms::{LocalRoomAgentInfo, LocalRoomParticipant, RoomTurnOutput};

struct FakeRoomParticipant {
    reply: String,
    hold_gate: Option<Gate>,
}

#[async_trait]
impl LocalRoomParticipant for FakeRoomParticipant {
    async fn run_room_turn(
        &self,
        req: residuum_rooms::LocalRoomTurnRequest,
    ) -> Result<RoomTurnOutput, String> {
        req.tool_status_tx.send(Some("thinking".to_owned())).ok();
        if let Some(gate) = &self.hold_gate {
            gate.entry().pass().await;
        }
        req.tool_status_tx.send(None).ok();
        Ok(RoomTurnOutput {
            content: self.reply.clone(),
            attachments: Vec::new(),
        })
    }

    async fn cancel_room_turn(&self, _room_id: &str) -> Result<(), String> {
        Ok(())
    }

    async fn close_room_session(&self, _room_id: &str) -> Result<(), String> {
        Ok(())
    }

    async fn generate_minutes(&self, prompt: String) -> Result<String, String> {
        Ok(format!(
            "# Meeting Minutes\n\n## Summary\nCaptured {} chars.\n\n## Decisions\n- Approved.\n\n## Action Items\n- None.\n",
            prompt.len()
        ))
    }
}

async fn next_room_frame(socket: &mut ClientSocket, expected_type: &str) -> Value {
    wait::guarded(format!("hub ws frame {expected_type}"), async {
        loop {
            let frame = next_frame(socket).await;
            if frame["type"] == expected_type {
                return frame;
            }
        }
    })
    .await
}

async fn wait_for_room_idle(socket: &mut ClientSocket, room_id: &str) {
    wait::guarded("room_turn_state idle", async {
        loop {
            let frame = next_frame(socket).await;
            if frame["type"] == "room_turn_state"
                && frame["room_id"] == room_id
                && frame["active_turn"].is_null()
            {
                return;
            }
        }
    })
    .await;
}

async fn stage_attachment(h: &Harness, filename: &str, body: &'static str) -> String {
    let (up_status, up_body) = h
        .send_body(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/hub/uploads?filename={filename}"))
                .header("content-type", "application/octet-stream")
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
    assert_eq!(up_status, StatusCode::OK);
    let upload_json: Value = serde_json::from_slice(&up_body).unwrap();
    upload_json["id"].as_str().unwrap().to_owned()
}

async fn create_test_room(h: &Harness, name: &str, topic: Option<&str>, pinned: bool) -> String {
    let created = h
        .expect(
            Method::POST,
            "/api/hub/rooms",
            Some(json!({
                "name": name,
                "topic": topic,
                "seat_ids": ["agent:scout"],
                "pinned": pinned
            })),
            StatusCode::CREATED,
        )
        .await;
    created["room"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn room_crud_lifecycle_ws_events_and_attachments() {
    let h = Harness::new();
    h.rooms.register_local_agent(LocalRoomAgentInfo {
        name: "scout".to_owned(),
        display_name: "Scout".to_owned(),
        role: Some("Research".to_owned()),
        participant: Arc::new(FakeRoomParticipant {
            reply: "All clear on my end.".to_owned(),
            hold_gate: None,
        }),
    });

    let addr = h.serve().await;
    let mut socket = connect_hub(addr).await;
    let snapshot = next_frame(&mut socket).await;
    assert_eq!(snapshot["type"], "agents_snapshot");

    assert_eq!(
        h.get_expect("/api/hub/rooms", StatusCode::OK).await["rooms"],
        json!([])
    );
    let cands = h
        .get_expect("/api/hub/rooms/candidates", StatusCode::OK)
        .await;
    assert_eq!(cands["candidates"][0]["seat_id"], "agent:scout");
    assert_eq!(cands["candidates"][0]["online"], true);

    h.expect(
        Method::POST,
        "/api/hub/rooms",
        Some(json!({ "name": "   ", "seat_ids": ["agent:scout"], "pinned": false })),
        StatusCode::BAD_REQUEST,
    )
    .await;

    let room_id = create_test_room(&h, "Ops Sync", Some("Status"), true).await;
    let ws_created = next_room_frame(&mut socket, "room_updated").await;
    assert_eq!(ws_created["room"]["id"], room_id);
    assert_eq!(ws_created["room"]["pinned"], true);

    let patched = h
        .expect(
            Method::PATCH,
            &format!("/api/hub/rooms/{room_id}"),
            Some(json!({ "topic": "" })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(patched["topic"], Value::Null);

    let posted = h
        .expect(
            Method::POST,
            &format!("/api/hub/rooms/{room_id}/messages"),
            Some(json!({ "content": "@Scout please review this", "upload_ids": [] })),
            StatusCode::ACCEPTED,
        )
        .await;
    assert_eq!(posted["kind"], "owner");

    let ws_owner_msg = next_room_frame(&mut socket, "room_message").await;
    assert_eq!(ws_owner_msg["message"]["kind"], "owner");
    let ws_agent_msg = next_room_frame(&mut socket, "room_message").await;
    assert_eq!(ws_agent_msg["message"]["kind"], "agent");
    assert_eq!(ws_agent_msg["message"]["content"], "All clear on my end.");
    wait_for_room_idle(&mut socket, &room_id).await;

    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/read"),
        Some(json!({ "seq": 2 })),
        StatusCode::NO_CONTENT,
    )
    .await;
    let detail = h
        .get_expect(
            &format!("/api/hub/rooms/{room_id}?limit=50"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(detail["room"]["unread_count"], 0);
    assert_eq!(detail["messages"].as_array().unwrap().len(), 2);

    let archived = h
        .post_expect(&format!("/api/hub/rooms/{room_id}/archive"), StatusCode::OK)
        .await;
    assert_eq!(archived["room"]["status"], "archived");
    assert!(archived["minutes_markdown"].as_str().is_some());
    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/messages"),
        Some(json!({ "content": "too late", "upload_ids": [] })),
        StatusCode::CONFLICT,
    )
    .await;

    let reopened = h
        .post_expect(&format!("/api/hub/rooms/{room_id}/reopen"), StatusCode::OK)
        .await;
    assert_eq!(reopened["status"], "active");

    h.expect(
        Method::DELETE,
        &format!("/api/hub/rooms/{room_id}"),
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    let ws_deleted = next_room_frame(&mut socket, "room_deleted").await;
    assert_eq!(ws_deleted["room_id"], room_id);
    h.get_expect(&format!("/api/hub/rooms/{room_id}"), StatusCode::NOT_FOUND)
        .await;
}

#[tokio::test]
async fn room_message_consumes_upload_and_serves_attachment() {
    let h = Harness::new();
    h.rooms.register_local_agent(LocalRoomAgentInfo {
        name: "scout".to_owned(),
        display_name: "Scout".to_owned(),
        role: None,
        participant: Arc::new(FakeRoomParticipant {
            reply: "Got the file.".to_owned(),
            hold_gate: None,
        }),
    });
    let room_id = create_test_room(&h, "Files Room", None, false).await;
    let upload_id = stage_attachment(&h, "notes.txt", "room attachment body").await;
    let posted = h
        .expect(
            Method::POST,
            &format!("/api/hub/rooms/{room_id}/messages"),
            Some(json!({ "content": "@Scout review", "upload_ids": [upload_id] })),
            StatusCode::ACCEPTED,
        )
        .await;
    assert_eq!(posted["attachments"][0]["filename"], "notes.txt");
    let att_url = posted["attachments"][0]["url"].as_str().unwrap();
    assert!(h.uploads.resolve(&upload_id).await.is_err());
    let (att_headers, att_bytes) = h.request_ok(Method::GET, att_url, "").await;
    assert_eq!(att_bytes, b"room attachment body");
    assert_eq!(att_headers["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn room_stop_endpoint_cancels_in_flight_turn() {
    struct ArcHeldParticipant(Arc<Gate>);
    #[async_trait]
    impl LocalRoomParticipant for ArcHeldParticipant {
        async fn run_room_turn(
            &self,
            _req: residuum_rooms::LocalRoomTurnRequest,
        ) -> Result<RoomTurnOutput, String> {
            self.0.entry().pass().await;
            Ok(RoomTurnOutput::default())
        }
        async fn cancel_room_turn(&self, _room_id: &str) -> Result<(), String> {
            self.0.open_all();
            Ok(())
        }
        async fn close_room_session(&self, _room_id: &str) -> Result<(), String> {
            Ok(())
        }
        async fn generate_minutes(&self, _prompt: String) -> Result<String, String> {
            Ok(String::new())
        }
    }

    let h = Harness::new();
    let shared_gate = Arc::new(Gate::closed("http-stop-gate"));

    h.rooms.register_local_agent(LocalRoomAgentInfo {
        name: "scout".to_owned(),
        display_name: "Scout".to_owned(),
        role: None,
        participant: Arc::new(ArcHeldParticipant(Arc::clone(&shared_gate))),
    });

    let created = h
        .expect(
            Method::POST,
            "/api/hub/rooms",
            Some(json!({
                "name": "Stop Test",
                "seat_ids": ["agent:scout"],
                "pinned": false
            })),
            StatusCode::CREATED,
        )
        .await;
    let room_id = created["room"]["id"].as_str().unwrap().to_owned();

    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/messages"),
        Some(json!({ "content": "@Scout start", "upload_ids": [] })),
        StatusCode::ACCEPTED,
    )
    .await;
    shared_gate.until_held(1).await;

    let stopped = h
        .post_expect(&format!("/api/hub/rooms/{room_id}/stop"), StatusCode::OK)
        .await;
    assert!(stopped["active_turn"].is_null());
}
