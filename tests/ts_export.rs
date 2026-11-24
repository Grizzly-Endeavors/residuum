#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]

use ts_rs::TS;

use residuum::background::team::{
    A2aVisibility, Actor, AgentErrorKind, AgentLastError, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome,
};
use residuum::checkpoints::{
    CheckpointDetail, CheckpointPage, RepoKind, RepoStats, RestoreOutcome, UndoOutcome,
};
use residuum::hub::mail::{
    MailAttachment, MailMessage, MailReplyResponse, MailSeen, MailSendResponse, MailSender,
    MailThread, MailThreadPage, MailThreadSummary,
};
use residuum::hub::overview::{AgentOverview, OverviewResponse};
use residuum::hub::push::{
    PatchPushDeviceRequest, PushDevice, PushDeviceList, PushDeviceResponse, PushEvent, PushFailure,
    PushKeyResponse, PushPayload, PushPreferences, PushPreferencesPatch, PushTestResult,
    PutPushDeviceRequest, WebPushSubscription, WebPushSubscriptionKeys,
};
use residuum::hub::team_events::{
    TeamEvent, TeamEventKind, TeamEventLevel, TeamEventPage, TeamEventPlace, TeamEventTarget,
};
use residuum::hub::types::{
    AgentActivity, AgentListResponse, AgentPatch, DeletedAgent, DeletedAgentListResponse,
    HubClientMessage, HubEvent, HubSocketFrame, NoticeLevel, RestoreAgentRequest,
};
use residuum::inference::{ImageData, MessageAttachment};
use residuum::interfaces::teams::setup_types::{
    AtkStatus, CleanupFailure, CleanupRequest, CleanupResult, CreatedResources, LogLine, LogStream,
    SetupError, SetupResult, SignInPrompt, SuggestedEndpointSource, TeamsSetupForm, TeamsSetupJob,
    TeamsSetupPhase, TeamsSetupPrereqs, TeamsSetupRedirect, TeamsSetupStart, TeamsSetupState,
    ToolStatus,
};
use residuum::pairing::types::{
    CreatePairingRequestBody, DeviceInfo, DeviceListResponse, HandoffBody, PairLinkResponse,
    PairedResponse, PairingPollResponse, PairingRequestCreated, PairingRequestStatus,
    PairingStateResponse, PendingPairingInfo, PollPairingBody, RecoveryCodeBody,
    RecoveryCodesResponse, RedeemTokenBody, WorkbenchHandoffResponse,
};
use residuum::protocol::{
    ActionInfo, ClientMessage, PulseInfo, ServerMessage, SessionListResponse, WorkbenchInfo,
};
use residuum::remote_access::status::{
    CertificateInfo, InstanceInfo, JoinProgress, JoinState, PendingJoinInfo, PinInfo,
    RemoteAccessState, RemoteAccessStatus, RemoteHosts, SiblingInfo,
};
use residuum_net::workbench::ArtifactSummary;

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "exports all protocol types sequentially into TypeScript"
)]
fn export_protocol_types() {
    let out_dir = std::env::var("RESIDUUM_TS_EXPORT_DIR")
        .unwrap_or_else(|_| "web/src/lib/generated".to_owned());
    let cfg = ts_rs::Config::new().with_out_dir(&out_dir);

    std::fs::create_dir_all(&out_dir).unwrap();

    ClientMessage::export_all(&cfg).unwrap();
    ServerMessage::export_all(&cfg).unwrap();
    ImageData::export_all(&cfg).unwrap();
    MessageAttachment::export_all(&cfg).unwrap();
    // HTTP response type for `GET /api/sessions` (its `SessionSummary`
    // items are also carried by the `session_started` frame).
    SessionListResponse::export_all(&cfg).unwrap();
    ArtifactSummary::export_all(&cfg).unwrap();
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
    HubEvent::export_all(&cfg).unwrap();
    HubSocketFrame::export_all(&cfg).unwrap();
    HubClientMessage::export_all(&cfg).unwrap();
    Actor::export_all(&cfg).unwrap();
    NoticeLevel::export_all(&cfg).unwrap();

    // Mail: `MailThreadPage` (with its `MailThreadSummary` items) answers
    // `GET /api/hub/mail/threads`, `MailThread` (with its `MailMessage`,
    // `MailSender`, `MailSeen` and `MailAttachment` parts) answers
    // `GET /api/hub/mail/threads/{ref}`, `MailSendResponse` answers
    // `POST /api/hub/mail/send`, and `MailReplyResponse` answers
    // `POST /api/hub/mail/threads/{ref}/reply`.
    MailThreadPage::export_all(&cfg).unwrap();
    MailThreadSummary::export_all(&cfg).unwrap();
    MailThread::export_all(&cfg).unwrap();
    MailMessage::export_all(&cfg).unwrap();
    MailSender::export_all(&cfg).unwrap();
    MailSeen::export_all(&cfg).unwrap();
    MailAttachment::export_all(&cfg).unwrap();
    MailSendResponse::export_all(&cfg).unwrap();
    MailReplyResponse::export_all(&cfg).unwrap();

    PairingStateResponse::export_all(&cfg).unwrap();
    CreatePairingRequestBody::export_all(&cfg).unwrap();
    PairingRequestCreated::export_all(&cfg).unwrap();
    PollPairingBody::export_all(&cfg).unwrap();
    PairingPollResponse::export_all(&cfg).unwrap();
    PairingRequestStatus::export_all(&cfg).unwrap();
    RedeemTokenBody::export_all(&cfg).unwrap();
    RecoveryCodeBody::export_all(&cfg).unwrap();
    HandoffBody::export_all(&cfg).unwrap();
    PairedResponse::export_all(&cfg).unwrap();
    DeviceListResponse::export_all(&cfg).unwrap();
    DeviceInfo::export_all(&cfg).unwrap();
    PendingPairingInfo::export_all(&cfg).unwrap();
    PairLinkResponse::export_all(&cfg).unwrap();
    RecoveryCodesResponse::export_all(&cfg).unwrap();
    WorkbenchHandoffResponse::export_all(&cfg).unwrap();

    RemoteAccessStatus::export_all(&cfg).unwrap();
    RemoteAccessState::export_all(&cfg).unwrap();
    RemoteHosts::export_all(&cfg).unwrap();
    CertificateInfo::export_all(&cfg).unwrap();
    PinInfo::export_all(&cfg).unwrap();
    InstanceInfo::export_all(&cfg).unwrap();
    SiblingInfo::export_all(&cfg).unwrap();
    PendingJoinInfo::export_all(&cfg).unwrap();
    JoinState::export_all(&cfg).unwrap();
    JoinProgress::export_all(&cfg).unwrap();

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

    TeamEventPage::export_all(&cfg).unwrap();
    TeamEvent::export_all(&cfg).unwrap();
    TeamEventKind::export_all(&cfg).unwrap();
    TeamEventLevel::export_all(&cfg).unwrap();
    TeamEventTarget::export_all(&cfg).unwrap();
    TeamEventPlace::export_all(&cfg).unwrap();

    OverviewResponse::export_all(&cfg).unwrap();
    AgentOverview::export_all(&cfg).unwrap();

    ToolStatus::export_all(&cfg).unwrap();
    AtkStatus::export_all(&cfg).unwrap();
    SuggestedEndpointSource::export_all(&cfg).unwrap();
    TeamsSetupPrereqs::export_all(&cfg).unwrap();
    TeamsSetupForm::export_all(&cfg).unwrap();
    TeamsSetupStart::export_all(&cfg).unwrap();
    TeamsSetupPhase::export_all(&cfg).unwrap();
    TeamsSetupState::export_all(&cfg).unwrap();
    LogStream::export_all(&cfg).unwrap();
    LogLine::export_all(&cfg).unwrap();
    SignInPrompt::export_all(&cfg).unwrap();
    SetupError::export_all(&cfg).unwrap();
    CreatedResources::export_all(&cfg).unwrap();
    SetupResult::export_all(&cfg).unwrap();
    TeamsSetupJob::export_all(&cfg).unwrap();
    TeamsSetupRedirect::export_all(&cfg).unwrap();
    CleanupRequest::export_all(&cfg).unwrap();
    CleanupFailure::export_all(&cfg).unwrap();
    CleanupResult::export_all(&cfg).unwrap();

    assert!(
        std::path::Path::new(&out_dir)
            .join("ClientMessage.ts")
            .exists(),
        "ClientMessage.ts should be generated"
    );
    assert!(
        std::path::Path::new(&out_dir)
            .join("ServerMessage.ts")
            .exists(),
        "ServerMessage.ts should be generated"
    );
    assert!(
        std::path::Path::new(&out_dir)
            .join("ImageAttachment.ts")
            .exists(),
        "ImageAttachment.ts should be generated (renamed from ImageData)"
    );
    assert!(
        std::path::Path::new(&out_dir)
            .join("TeamsSetupJob.ts")
            .exists(),
        "TeamsSetupJob.ts should be generated"
    );
    assert!(
        std::path::Path::new(&out_dir)
            .join("TeamsSetupPrereqs.ts")
            .exists(),
        "TeamsSetupPrereqs.ts should be generated"
    );
}
