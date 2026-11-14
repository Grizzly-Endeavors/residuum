//! Multi-agent meeting rooms: wire types, validation, mention/budget math,
//! on-disk persistence (`<hub_dir>/rooms/<room_id>/`), turn coordinator,
//! System 1 speaker selector, A2A participant driver, and agent room view.

#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;

pub mod agent_view;
pub mod mentions;
pub mod participants;
pub mod selector;
pub mod service;
pub mod store;
pub mod types;

pub use agent_view::{RoomAgentView, RoomReadResult};
pub use mentions::{parse_agent_baton_mentions, parse_owner_mentions};
pub use participants::{
    A2aRoomDriver, LocalRoomParticipant, LocalRoomTurnRequest, RemoteRoomDriver,
    RemoteRoomTurnRequest, RoomTurnOutput,
};
pub use selector::{SelectorOutcome, SpeakerSelector, SystemOneSpeakerSelector};
pub use service::{LocalRoomAgentInfo, RoomEvent, RoomService, RoomServiceError};
pub use store::{NewRoomMessage, RoomStore, RoomStoreError};
pub use types::{
    CreateRoomRequest, MAX_ROOM_NAME_CHARS, MAX_SEAT_TURNS_PER_WINDOW, MarkRoomReadRequest,
    PatchRoomRequest, PostRoomMessageRequest, RoomActiveTurn, RoomAttachment,
    RoomCandidatesResponse, RoomDetail, RoomDetailResponse, RoomId, RoomListResponse, RoomMessage,
    RoomMessageKind, RoomMinutesMeta, RoomRecord, RoomRosterCandidate, RoomSeat, RoomSeatKind,
    RoomStatus, RoomSummary, RoomValidationError, RoomWindowState, generate_message_id,
    generate_room_id, validate_room_id, validate_room_name, window_cap_for_roster,
};
