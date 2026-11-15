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

#[tokio::test]
async fn hub_ws_suppresses_room_events_through_artifacts_origin() {
    let h = Harness::new();
    h.rooms.register_local_agent(LocalRoomAgentInfo {
        name: "scout".to_owned(),
        display_name: "Scout".to_owned(),
        role: None,
        participant: Arc::new(FakeRoomParticipant {
            reply: "Quiet room reply.".to_owned(),
            hold_gate: None,
        }),
    });

    let normal_addr = h.serve().await;
    let mut normal_socket = connect_hub(normal_addr).await;
    assert_eq!(
        next_frame(&mut normal_socket).await["type"],
        "agents_snapshot"
    );

    let artifacts_app = h.app.clone().layer(axum::middleware::from_fn(
        |mut req: Request<Body>, next: axum::middleware::Next| async move {
            req.extensions_mut()
                .insert(residuum_net::workbench::ArtifactsOrigin);
            next.run(req).await
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let artifacts_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, artifacts_app).await.ok();
    });

    let mut artifacts_socket = connect_hub(artifacts_addr).await;
    assert_eq!(
        next_frame(&mut artifacts_socket).await["type"],
        "agents_snapshot"
    );

    let room_id = create_test_room(&h, "Private Sync", None, false).await;
    let ws_created = next_room_frame(&mut normal_socket, "room_updated").await;
    assert_eq!(ws_created["room"]["id"], room_id);

    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/messages"),
        Some(json!({ "content": "@Scout report", "upload_ids": [] })),
        StatusCode::ACCEPTED,
    )
    .await;
    let ws_msg = next_room_frame(&mut normal_socket, "room_message").await;
    assert_eq!(ws_msg["room_id"], room_id);
    wait_for_room_idle(&mut normal_socket, &room_id).await;

    h.directory
        .events
        .send(HubEvent::notice(
            crate::types::NoticeLevel::Info,
            "hub-level notice".to_owned(),
            None,
        ))
        .unwrap();
    send_client(&mut artifacts_socket, &json!({ "type": "ping" })).await;

    let mut saw_notice = false;
    let mut saw_pong = false;
    wait::guarded("non-room notice and pong on artifacts socket", async {
        while !(saw_notice && saw_pong) {
            let frame = next_frame(&mut artifacts_socket).await;
            let frame_type = frame["type"].as_str().unwrap_or("");
            assert!(
                !frame_type.starts_with("room_"),
                "artifacts-origin ws must not receive room frames, got: {frame}"
            );
            if frame_type == "notice" && frame["message"] == "hub-level notice" {
                saw_notice = true;
            } else if frame_type == "pong" {
                saw_pong = true;
            }
        }
    })
    .await;
}

async fn post_and_wait_idle(h: &Harness, room_id: &str, content: &str, expected_seq: u64) {
    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/messages"),
        Some(json!({ "content": content, "upload_ids": [] })),
        StatusCode::ACCEPTED,
    )
    .await;
    wait::guarded("room idle after message", async {
        loop {
            let detail = h
                .get_expect(
                    &format!("/api/hub/rooms/{room_id}?limit=10"),
                    StatusCode::OK,
                )
                .await;
            if detail["room"]["last_seq"] == expected_seq && detail["room"]["active_turn"].is_null()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/read"),
        Some(json!({ "seq": expected_seq })),
        StatusCode::NO_CONTENT,
    )
    .await;
}

#[tokio::test]
async fn capture_notes_endpoint_accepts_empty_and_explicit_mode_and_rejects_no_new_messages() {
    let h = Harness::new();
    h.rooms.register_local_agent(LocalRoomAgentInfo {
        name: "scout".to_owned(),
        display_name: "Scout".to_owned(),
        role: Some("Research".to_owned()),
        participant: Arc::new(FakeRoomParticipant {
            reply: "First round reply.".to_owned(),
            hold_gate: None,
        }),
    });

    let room_id = create_test_room(&h, "Kitchen Table", Some("Weekly"), true).await;

    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/capture-notes"),
        Some(json!({})),
        StatusCode::CONFLICT,
    )
    .await;

    post_and_wait_idle(&h, &room_id, "@Scout status?", 2).await;

    let first = h
        .post_expect(
            &format!("/api/hub/rooms/{room_id}/capture-notes"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(
        first["room"]["status"], "active",
        "capture-notes must leave room active"
    );
    assert_eq!(
        first["room"]["meetings"].as_array().map(Vec::len),
        Some(1),
        "first capture must record 1 meeting"
    );
    assert_eq!(
        first["room"]["last_captured_seq"], 3,
        "last_captured_seq must include divider seq 3"
    );
    assert_eq!(
        first["room"]["unread_count"], 0,
        "capture-notes must not produce unread badge"
    );
    assert!(
        first["minutes_markdown"].as_str().is_some(),
        "capture-notes must return minutes_markdown"
    );

    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/capture-notes"),
        Some(json!({ "mode": "new" })),
        StatusCode::CONFLICT,
    )
    .await;
    h.expect(
        Method::POST,
        &format!("/api/hub/rooms/{room_id}/capture-notes"),
        Some(json!({ "mode": "bogus" })),
        StatusCode::BAD_REQUEST,
    )
    .await;

    post_and_wait_idle(&h, &room_id, "@Scout second round?", 5).await;

    let second = h
        .expect(
            Method::POST,
            &format!("/api/hub/rooms/{room_id}/capture-notes"),
            Some(json!({ "mode": "new" })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(
        second["room"]["meetings"].as_array().map(Vec::len),
        Some(2),
        "second capture must record 2 meetings"
    );
    assert_ne!(
        second["room"]["meetings"][0]["shared_path"], second["room"]["meetings"][1]["shared_path"],
        "second capture must allocate a distinct shared_path"
    );
}
