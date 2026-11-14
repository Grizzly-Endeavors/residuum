//! Multi-agent meeting rooms: wire types, validation, mention/budget math, and
//! on-disk persistence (`<hub_dir>/rooms/<room_id>/`).

pub mod mentions;
pub mod store;
pub mod types;

pub use mentions::{parse_agent_baton_mentions, parse_owner_mentions};
pub use store::{NewRoomMessage, RoomStore, RoomStoreError};
pub use types::{
    CreateRoomRequest, MAX_ROOM_NAME_CHARS, MAX_SEAT_TURNS_PER_WINDOW, MarkRoomReadRequest,
    PatchRoomRequest, PostRoomMessageRequest, RoomActiveTurn, RoomAttachment,
    RoomCandidatesResponse, RoomDetail, RoomDetailResponse, RoomId, RoomListResponse, RoomMessage,
    RoomMessageKind, RoomMinutesMeta, RoomRecord, RoomRosterCandidate, RoomSeat, RoomSeatKind,
    RoomStatus, RoomSummary, RoomValidationError, RoomWindowState, generate_message_id,
    generate_room_id, validate_room_id, validate_room_name, window_cap_for_roster,
};
