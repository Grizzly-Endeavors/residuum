//! Generates TypeScript type definitions from Rust protocol types via ts-rs.
//!
//! Running `cargo test` produces `.ts` files in `web/src/lib/generated/`.
//! These files are committed to git so the frontend can import them directly.

#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod ts_export {
    use ts_rs::TS;

    use residuum::checkpoints::{
        CheckpointDetail, CheckpointPage, RepoKind, RepoStats, RestoreOutcome, UndoOutcome,
    };
    use residuum::gateway::protocol::{
        ActionInfo, ArtifactSummary, ClientMessage, PulseInfo, ServerMessage, SessionListResponse,
        WorkbenchInfo,
    };
    use residuum::hub::inbox::{HubInboxItem, HubInboxPage, HubInboxUnread, InboxStatus};
    use residuum::hub::overview::{AgentOverview, OverviewResponse};
    use residuum::hub::push::{
        PatchPushDeviceRequest, PushDevice, PushDeviceList, PushDeviceResponse, PushEvent,
        PushFailure, PushKeyResponse, PushPayload, PushPreferences, PushPreferencesPatch,
        PushTestResult, PutPushDeviceRequest, WebPushSubscription, WebPushSubscriptionKeys,
    };
    use residuum::hub::team_events::{
        TeamEvent, TeamEventKind, TeamEventLevel, TeamEventPage, TeamEventPlace, TeamEventTarget,
    };
    use residuum::hub::types::{
        A2aVisibility, Actor, AgentActivity, AgentErrorKind, AgentLastError, AgentListResponse,
        AgentPatch, AgentState, AgentSummary, CreateAgentRequest, DeleteOutcome, DeletedAgent,
        DeletedAgentListResponse, HubClientMessage, HubEvent, HubSocketFrame, NoticeLevel,
        RestoreAgentRequest,
    };
    use residuum::inbox::InboxAttachment;
    use residuum::inference::ImageData;

    #[test]
    fn export_protocol_types() {
        let out_dir = "web/src/lib/generated";
        let cfg = ts_rs::Config::new().with_out_dir(out_dir);

        // Ensure the output directory exists
        std::fs::create_dir_all(out_dir).unwrap();

        // Export each type — dependencies are exported transitively
        ClientMessage::export_all(&cfg).unwrap();
        ServerMessage::export_all(&cfg).unwrap();
        ImageData::export_all(&cfg).unwrap();
        // HTTP response type for `GET /api/sessions` (its `SessionSummary`
        // items are also carried by the `session_started` frame).
        SessionListResponse::export_all(&cfg).unwrap();
        // HTTP response items for `GET /api/team/workbench/artifacts`.
        ArtifactSummary::export_all(&cfg).unwrap();
        // `GET /api/team/workbench/info` (its relay origins are exported with it).
        WorkbenchInfo::export_all(&cfg).unwrap();
        // The checkpoints API: `GET /api/checkpoints` (its `CheckpointSummary`
        // items and `CheckpointTrigger` are exported with it), `GET
        // /api/checkpoints/{id}` (its `ChangedPath`/`ChangeKind` are exported
        // with it), `GET /api/checkpoints/stats`, and the `POST
        // .../restore`/`.../undo` responses. `RepoKind` is the `repo` query
        // parameter/request-body field every checkpoints route takes.
        CheckpointPage::export_all(&cfg).unwrap();
        CheckpointDetail::export_all(&cfg).unwrap();
        RepoStats::export_all(&cfg).unwrap();
        RestoreOutcome::export_all(&cfg).unwrap();
        UndoOutcome::export_all(&cfg).unwrap();
        RepoKind::export_all(&cfg).unwrap();
        // `GET /api/scheduled/pulses` and `GET /api/scheduled/actions`.
        PulseInfo::export_all(&cfg).unwrap();
        ActionInfo::export_all(&cfg).unwrap();

        // The hub API: `AgentSummary` (with `AgentState`, `AgentLastError` and
        // `A2aVisibility`) is the item of `GET /api/hub/agents` and the
        // `agent` of the hub WebSocket's frames; `CreateAgentRequest` and
        // `AgentPatch` are the bodies of `POST /api/hub/agents` and `PATCH
        // /api/hub/agents/{name}`; `DeleteOutcome` answers the `DELETE`;
        // `DeletedAgent` is the item of `GET /api/hub/agents/deleted` and
        // `RestoreAgentRequest` the body of `POST /api/hub/agents/restore`;
        // `AgentActivity` is the `busy`/`busy_since`/`unread` set of
        // `agent_activity` and the snapshot's `activity` map.
        AgentSummary::export_all(&cfg).unwrap();
        AgentState::export_all(&cfg).unwrap();
        AgentLastError::export_all(&cfg).unwrap();
        A2aVisibility::export_all(&cfg).unwrap();
        CreateAgentRequest::export_all(&cfg).unwrap();
        AgentPatch::export_all(&cfg).unwrap();
        DeleteOutcome::export_all(&cfg).unwrap();
        DeletedAgent::export_all(&cfg).unwrap();
        RestoreAgentRequest::export_all(&cfg).unwrap();
        AgentActivity::export_all(&cfg).unwrap();
        AgentErrorKind::export_all(&cfg).unwrap();
        // The hub's list envelopes: `GET /api/hub/agents` and `GET
        // /api/hub/agents/deleted`.
        AgentListResponse::export_all(&cfg).unwrap();
        DeletedAgentListResponse::export_all(&cfg).unwrap();
        // The hub WebSocket: `HubEvent` is every frame the hub forwards from
        // its bus (with `Actor` and `NoticeLevel`), `HubSocketFrame` the two it
        // sends on its own (`hub_boot` and `agents_snapshot`), and
        // `HubClientMessage` the one message a client sends. The team change
        // frames are the agent protocol's `ServerMessage` variants. The client
        // message is exported under this name so it doesn't collide with the
        // agent protocol's `ClientMessage`.
        HubEvent::export_all(&cfg).unwrap();
        HubSocketFrame::export_all(&cfg).unwrap();
        HubClientMessage::export_all(&cfg).unwrap();
        Actor::export_all(&cfg).unwrap();
        NoticeLevel::export_all(&cfg).unwrap();

        // The cross-agent inbox: `HubInboxPage` (with its `HubInboxItem` and
        // `InboxAttachment` items) answers `GET /api/hub/inbox`,
        // `HubInboxUnread` answers `GET /api/hub/inbox/unread`, and
        // `HubInboxItem` is the `item` of the per-item routes. `InboxStatus`
        // is the `status` query parameter.
        HubInboxPage::export_all(&cfg).unwrap();
        HubInboxItem::export_all(&cfg).unwrap();
        InboxAttachment::export_all(&cfg).unwrap();
        HubInboxUnread::export_all(&cfg).unwrap();
        InboxStatus::export_all(&cfg).unwrap();

        // Web Push: `PushKeyResponse` answers `GET /api/hub/push/key`,
        // `PushDeviceList` answers `GET /api/hub/push/devices` (with its
        // `PushDevice` items, their `PushPreferences` and `PushFailure`),
        // `PutPushDeviceRequest` is the body of `PUT /api/hub/push/devices`
        // (with the browser's `WebPushSubscription` and its keys, and a
        // `PushPreferencesPatch`), `PatchPushDeviceRequest` the body of
        // `PATCH /api/hub/push/devices/{id}`, `PushDeviceResponse` the answer
        // of both, and `PushTestResult` the answer of `POST
        // /api/hub/push/devices/{id}/test`. `PushPayload` (with `PushEvent`)
        // is the JSON the service worker decrypts from a push message.
        PushKeyResponse::export_all(&cfg).unwrap();
        PushDeviceList::export_all(&cfg).unwrap();
        PushDevice::export_all(&cfg).unwrap();
        PushPreferences::export_all(&cfg).unwrap();
        PushPreferencesPatch::export_all(&cfg).unwrap();
        PushFailure::export_all(&cfg).unwrap();
        PutPushDeviceRequest::export_all(&cfg).unwrap();
        PatchPushDeviceRequest::export_all(&cfg).unwrap();
        WebPushSubscription::export_all(&cfg).unwrap();
        WebPushSubscriptionKeys::export_all(&cfg).unwrap();
        PushDeviceResponse::export_all(&cfg).unwrap();
        PushTestResult::export_all(&cfg).unwrap();
        PushPayload::export_all(&cfg).unwrap();
        PushEvent::export_all(&cfg).unwrap();

        // The team event log: `TeamEventPage` (with its `TeamEvent` items and
        // their `TeamEventKind`, `TeamEventLevel` and `TeamEventTarget`)
        // answers `GET /api/hub/events`, and `TeamEvent` is the `event` of the
        // hub WebSocket's `team_event` frame. `TeamEventPlace` is the `place`
        // of an `agent_place` target.
        TeamEventPage::export_all(&cfg).unwrap();
        TeamEvent::export_all(&cfg).unwrap();
        TeamEventKind::export_all(&cfg).unwrap();
        TeamEventLevel::export_all(&cfg).unwrap();
        TeamEventTarget::export_all(&cfg).unwrap();
        TeamEventPlace::export_all(&cfg).unwrap();

        // The team overview: `OverviewResponse` (with its `AgentOverview`
        // items and what they hold) answers `GET /api/hub/overview`, and
        // `AgentOverview` is the `overview` of the hub WebSocket's
        // `agent_overview` frame.
        OverviewResponse::export_all(&cfg).unwrap();
        AgentOverview::export_all(&cfg).unwrap();

        // Verify the generated files exist
        assert!(
            std::path::Path::new("web/src/lib/generated/ClientMessage.ts").exists(),
            "ClientMessage.ts should be generated"
        );
        assert!(
            std::path::Path::new("web/src/lib/generated/ServerMessage.ts").exists(),
            "ServerMessage.ts should be generated"
        );
        assert!(
            std::path::Path::new("web/src/lib/generated/ImageAttachment.ts").exists(),
            "ImageAttachment.ts should be generated (renamed from ImageData)"
        );
    }
}
