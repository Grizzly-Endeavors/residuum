//! Wire and persistence types for multi-agent meeting rooms (`#860`).

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use ts_rs::TS;

/// Maximum length in characters for a room's display name.
pub const MAX_ROOM_NAME_CHARS: usize = 120;

/// Maximum number of turns any single seat may take within one turn window.
pub const MAX_SEAT_TURNS_PER_WINDOW: u32 = 3;

/// Validation errors for room identifiers and metadata.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RoomValidationError {
    /// The room ID does not match `^room_[a-z0-9]{8,32}$`.
    #[error(
        "invalid room id '{0}': expected 'room_' followed by 8 to 32 lowercase alphanumeric characters"
    )]
    InvalidRoomId(String),
    /// The room name is empty after trimming or exceeds [`MAX_ROOM_NAME_CHARS`].
    #[error("invalid room name: {0}")]
    InvalidRoomName(String),
}

/// A validated meeting room identifier (`^room_[a-z0-9]{8,32}$`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
#[ts(export, type = "string")]
pub struct RoomId(String);

impl Serialize for RoomId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RoomId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

impl RoomId {
    /// Validate and construct a [`RoomId`] from any string-like value.
    ///
    /// # Errors
    /// Returns [`RoomValidationError::InvalidRoomId`] if `id` does not match
    /// `^room_[a-z0-9]{8,32}$`.
    pub fn parse(id: impl Into<String>) -> Result<Self, RoomValidationError> {
        let raw = id.into();
        validate_room_id(&raw)?;
        Ok(Self(raw))
    }

    /// Borrow the underlying string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for RoomId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for RoomId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for RoomId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoomId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<RoomId> for String {
    fn from(id: RoomId) -> Self {
        id.0
    }
}

impl TryFrom<String> for RoomId {
    type Error = RoomValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl FromStr for RoomId {
    type Err = RoomValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PartialEq<str> for RoomId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for RoomId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for RoomId {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

/// Validate a room ID against `^room_[a-z0-9]{8,32}$`.
///
/// # Errors
/// Returns [`RoomValidationError::InvalidRoomId`] if the ID does not match.
pub fn validate_room_id(id: &str) -> Result<(), RoomValidationError> {
    let Some(suffix) = id.strip_prefix("room_") else {
        return Err(RoomValidationError::InvalidRoomId(id.to_owned()));
    };
    let valid_len = (8..=32).contains(&suffix.len());
    let valid_chars = suffix
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if valid_len && valid_chars {
        Ok(())
    } else {
        Err(RoomValidationError::InvalidRoomId(id.to_owned()))
    }
}

const BASE36_CHARS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

fn encode_base36_suffix() -> String {
    let millis = u64::try_from(Utc::now().timestamp_millis().max(0)).unwrap_or(0);
    let mut ts_buf = [b'0'; 8];
    let mut val = millis;
    for slot in ts_buf.iter_mut().rev() {
        let digit = usize::try_from(val % 36).unwrap_or(0);
        *slot = BASE36_CHARS.get(digit).copied().unwrap_or(b'0');
        val /= 36;
    }
    let mut out = String::with_capacity(14);
    for b in ts_buf {
        out.push(char::from(b));
    }
    let mut rng = rand::thread_rng();
    for _ in 0..6 {
        let idx = rand::Rng::gen_range(&mut rng, 0..BASE36_CHARS.len());
        let b = BASE36_CHARS.get(idx).copied().unwrap_or(b'0');
        out.push(char::from(b));
    }
    out
}

/// Generate a fresh [`RoomId`] formatted as `"room_" + 14 lowercase base36 chars`.
#[must_use]
pub fn generate_room_id() -> RoomId {
    RoomId(format!("room_{}", encode_base36_suffix()))
}

/// Generate a fresh message ID formatted as `"rmsg_" + 14 lowercase base36 chars`.
#[must_use]
pub fn generate_message_id() -> String {
    format!("rmsg_{}", encode_base36_suffix())
}

/// Validate and normalize a room display name (trimmed, non-empty, `<= 120` chars).
///
/// # Errors
/// Returns [`RoomValidationError::InvalidRoomName`] if empty or too long.
pub fn validate_room_name(name: &str) -> Result<String, RoomValidationError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(RoomValidationError::InvalidRoomName(
            "room name must not be empty".to_owned(),
        ));
    }
    if trimmed.chars().count() > MAX_ROOM_NAME_CHARS {
        return Err(RoomValidationError::InvalidRoomName(format!(
            "room name must be at most {MAX_ROOM_NAME_CHARS} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Compute the per-owner-message agent turn window cap `ceil(1.5 * roster_size)`.
#[must_use]
pub fn window_cap_for_roster(roster_size: usize) -> u32 {
    let scaled = roster_size.saturating_mul(3).div_ceil(2);
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

/// Lifecycle status of a meeting room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RoomStatus {
    /// Open for new messages and turns.
    Active,
    /// Read-only archive; transcripts and minutes remain readable.
    Archived,
}

/// Where a room participant seat is hosted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RoomSeatKind {
    /// Hosted locally on this Residuum hub (`agent:<name>`).
    Local,
    /// Joined sibling Residuum hub A2A agent (`a2a:<instance>/<agent>`).
    SiblingA2a,
    /// Configured external A2A agent (`a2a:<name>`).
    ExternalA2a,
}

/// One seat on a room's active roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomSeat {
    /// Canonical seat identifier (`agent:<name>`, `a2a:<instance>/<name>`, or `a2a:<name>`).
    pub seat_id: String,
    /// Seat hosting kind.
    pub kind: RoomSeatKind,
    /// Human-readable name displayed in transcripts and used for `@Name` mentions.
    pub display_name: String,
    /// Short role description shown to the System 1 speaker selector and UI.
    #[serde(default)]
    pub role: Option<String>,
    /// Sibling hub instance slug when `kind` is [`RoomSeatKind::SiblingA2a`].
    #[serde(default)]
    pub instance: Option<String>,
}

impl RoomSeat {
    /// Leaf handle for `@mention` matching (e.g. `"scout"` for `"agent:scout"` or
    /// `"planner"` for `"a2a:ops/planner"`).
    #[must_use]
    pub fn leaf_slug(&self) -> &str {
        let stripped = self.full_slug();
        match stripped.rsplit_once('/') {
            Some((_, leaf)) => leaf,
            None => stripped,
        }
    }

    /// Full seat slug without its `agent:` or `a2a:` scheme prefix.
    #[must_use]
    pub fn full_slug(&self) -> &str {
        self.seat_id
            .strip_prefix("agent:")
            .or_else(|| self.seat_id.strip_prefix("a2a:"))
            .unwrap_or(&self.seat_id)
    }
}

/// Current turn-budget and queue state for an active room.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomWindowState {
    /// Active roster count when the current turn window began.
    #[ts(type = "number")]
    pub roster_size_at_start: usize,
    /// Maximum agent turns allowed in this window (`ceil(1.5 * R)`).
    pub window_cap: u32,
    /// Agent turns completed so far in this window.
    pub window_used: u32,
    /// Completed turns per `seat_id` in this window (capped at [`MAX_SEAT_TURNS_PER_WINDOW`]).
    #[serde(default)]
    pub per_seat_used: BTreeMap<String, u32>,
    /// Seat IDs explicitly queued to speak next (e.g. from `@mention` baton passes).
    #[serde(default)]
    pub queued_seats: Vec<String>,
    /// True when more turns were wanted or queued at `window_used >= window_cap`.
    #[serde(default)]
    pub paused_at_cap: bool,
    /// True when the room fell back to `@mention`-only mode because `SystemOne` was unavailable.
    #[serde(default)]
    pub decision_model_fallback: bool,
}

impl RoomWindowState {
    /// Initialize a fresh turn window for a roster of `roster_size` seats.
    #[must_use]
    pub fn fresh(roster_size: usize) -> Self {
        Self {
            roster_size_at_start: roster_size,
            window_cap: window_cap_for_roster(roster_size),
            window_used: 0,
            per_seat_used: BTreeMap::new(),
            queued_seats: Vec::new(),
            paused_at_cap: false,
            decision_model_fallback: false,
        }
    }
}

/// Metadata tracking meeting minutes generated on room archive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomMinutesMeta {
    /// Workspace-relative path under `team/minutes/` if minutes have been written.
    pub shared_path: Option<String>,
    /// Highest message `seq` covered by the current minutes (`0` if never archived with messages).
    #[ts(type = "number")]
    pub archived_at_seq: u64,
    /// Inbox item ref created when minutes were delivered to the owner's inbox.
    pub inbox_ref: Option<String>,
    /// When the minutes were last generated or updated.
    #[ts(type = "string | null")]
    pub updated_at: Option<DateTime<Utc>>,
}

/// Persistent `room.json` record for a meeting room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomRecord {
    /// Validated room identifier.
    pub id: RoomId,
    /// Human-readable room title.
    pub name: String,
    /// Optional room agenda/topic.
    pub topic: Option<String>,
    /// Active vs archived lifecycle status.
    pub status: RoomStatus,
    /// Whether the room is pinned under `Team -> Rooms` in the left rail.
    pub pinned: bool,
    /// Creation timestamp (UTC).
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
    /// Last metadata or message update timestamp (UTC).
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
    /// Timestamp when the room was archived, or `None` when active.
    #[ts(type = "string | null")]
    pub archived_at: Option<DateTime<Utc>>,
    /// Currently active roster seats.
    pub roster: Vec<RoomSeat>,
    /// All seat IDs that are or were ever on this room's roster.
    #[serde(default)]
    pub historical_participants: Vec<String>,
    /// Highest sequence number written to `transcript.jsonl` (`0` if empty).
    #[ts(type = "number")]
    pub last_seq: u64,
    /// Highest sequence number marked read by the owner.
    #[ts(type = "number")]
    pub last_read_seq: u64,
    /// Highest sequence number delivered to each remote seat for catch-up deltas.
    #[serde(default)]
    #[ts(type = "{ [key in string]: number }")]
    pub last_seen_seq: BTreeMap<String, u64>,
    /// Current turn window and queue state.
    #[serde(default)]
    pub window: RoomWindowState,
    /// Generated meeting minutes metadata.
    #[serde(default)]
    pub minutes: RoomMinutesMeta,
}

impl RoomRecord {
    /// Number of transcript messages after `last_read_seq`.
    #[must_use]
    pub fn unread_count(&self) -> u64 {
        self.last_seq.saturating_sub(self.last_read_seq)
    }

    /// Whether `seat_id` is currently on the active roster or was ever a participant.
    #[must_use]
    pub fn is_participant(&self, seat_id: &str) -> bool {
        self.roster.iter().any(|seat| seat.seat_id == seat_id)
            || self.historical_participants.iter().any(|id| id == seat_id)
    }
}

/// Compact room entry returned in `GET /api/hub/rooms` and `HubEvent::RoomUpdated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomSummary {
    /// Room ID.
    pub id: RoomId,
    /// Room title.
    pub name: String,
    /// Optional topic.
    pub topic: Option<String>,
    /// Active or archived.
    pub status: RoomStatus,
    /// Pinned in left rail.
    pub pinned: bool,
    /// Creation timestamp (UTC).
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
    /// Last update timestamp (UTC).
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
    /// Archive timestamp (UTC), if archived.
    #[ts(type = "string | null")]
    pub archived_at: Option<DateTime<Utc>>,
    /// Active roster seats.
    pub roster: Vec<RoomSeat>,
    /// Latest message sequence number.
    #[ts(type = "number")]
    pub last_seq: u64,
    /// Unread message count (`last_seq - last_read_seq`).
    #[ts(type = "number")]
    pub unread_count: u64,
    /// True when the current turn window paused at its cap.
    pub paused_at_cap: bool,
    /// Workspace-relative shared minutes path if generated.
    pub minutes_shared_path: Option<String>,
}

impl RoomSummary {
    /// Project a [`RoomRecord`] into a [`RoomSummary`].
    #[must_use]
    pub fn from_record(record: &RoomRecord) -> Self {
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            topic: record.topic.clone(),
            status: record.status,
            pinned: record.pinned,
            created_at: record.created_at,
            updated_at: record.updated_at,
            archived_at: record.archived_at,
            roster: record.roster.clone(),
            last_seq: record.last_seq,
            unread_count: record.unread_count(),
            paused_at_cap: record.window.paused_at_cap,
            minutes_shared_path: record.minutes.shared_path.clone(),
        }
    }
}

/// Full room state returned by room detail, patch, continue, stop, and reopen routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomDetail {
    /// Room ID.
    pub id: RoomId,
    /// Room title.
    pub name: String,
    /// Optional topic.
    pub topic: Option<String>,
    /// Active or archived.
    pub status: RoomStatus,
    /// Pinned in left rail.
    pub pinned: bool,
    /// Creation timestamp (UTC).
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
    /// Last update timestamp (UTC).
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
    /// Archive timestamp (UTC), if archived.
    #[ts(type = "string | null")]
    pub archived_at: Option<DateTime<Utc>>,
    /// Active roster seats.
    pub roster: Vec<RoomSeat>,
    /// All current and past participating seat IDs.
    pub historical_participants: Vec<String>,
    /// Latest message sequence number.
    #[ts(type = "number")]
    pub last_seq: u64,
    /// Last sequence number read by owner.
    #[ts(type = "number")]
    pub last_read_seq: u64,
    /// Unread message count (`last_seq - last_read_seq`).
    #[ts(type = "number")]
    pub unread_count: u64,
    /// Current turn window state.
    pub window: RoomWindowState,
    /// Meeting minutes metadata.
    pub minutes: RoomMinutesMeta,
    /// In-flight speaker turn if a seat is currently thinking/running tools.
    pub active_turn: Option<RoomActiveTurn>,
}

impl RoomDetail {
    /// Project a [`RoomRecord`] plus optional live [`RoomActiveTurn`] into a [`RoomDetail`].
    #[must_use]
    pub fn from_record(record: &RoomRecord, active_turn: Option<RoomActiveTurn>) -> Self {
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            topic: record.topic.clone(),
            status: record.status,
            pinned: record.pinned,
            created_at: record.created_at,
            updated_at: record.updated_at,
            archived_at: record.archived_at,
            roster: record.roster.clone(),
            historical_participants: record.historical_participants.clone(),
            last_seq: record.last_seq,
            last_read_seq: record.last_read_seq,
            unread_count: record.unread_count(),
            window: record.window.clone(),
            minutes: record.minutes.clone(),
            active_turn,
        }
    }
}

/// Who authored a [`RoomMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RoomMessageKind {
    /// Human owner message.
    Owner,
    /// Local or A2A agent reply.
    Agent,
    /// Inline system notice (e.g. offline/errored seat halt).
    System,
}

/// File or image attachment carried on a [`RoomMessage`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomAttachment {
    /// Attachment or upload identifier.
    pub id: String,
    /// Original filename.
    pub filename: String,
    /// MIME type.
    pub mime_type: String,
    /// File size in bytes.
    #[ts(type = "number")]
    pub size: u64,
    /// URL serving the attachment bytes.
    pub url: String,
}

/// One persisted entry in `<rooms_root>/<room_id>/transcript.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomMessage {
    /// 1-based monotonic sequence number within this room.
    #[ts(type = "number")]
    pub seq: u64,
    /// Message ID (`rmsg_...`).
    pub id: String,
    /// Parent room ID.
    pub room_id: RoomId,
    /// Author kind (`owner`, `agent`, `system`).
    pub kind: RoomMessageKind,
    /// Authoring seat ID for agent messages (`None` for owner/system).
    pub seat_id: Option<String>,
    /// Display name shown in the transcript.
    pub author_name: String,
    /// Hosting kind of the authoring seat (`None` for owner/system).
    pub seat_kind: Option<RoomSeatKind>,
    /// Message markdown text.
    pub content: String,
    /// File/image attachments on this message.
    #[serde(default)]
    pub attachments: Vec<RoomAttachment>,
    /// UTC timestamp when the message was appended.
    #[ts(type = "string")]
    pub timestamp: DateTime<Utc>,
}

/// Currently executing seat turn inside a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomActiveTurn {
    /// Seat currently taking a turn.
    pub seat_id: String,
    /// Display name shown in the typing indicator.
    pub display_name: String,
    /// Seat hosting kind.
    pub seat_kind: RoomSeatKind,
    /// When the turn started (UTC).
    #[ts(type = "string")]
    pub started_at: DateTime<Utc>,
    /// Live tool call description for local agents, if executing a tool.
    pub tool_status: Option<String>,
}

/// Eligible seat returned by `GET /api/hub/rooms/candidates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomRosterCandidate {
    /// Canonical seat ID.
    pub seat_id: String,
    /// Seat hosting kind.
    pub kind: RoomSeatKind,
    /// Display name.
    pub display_name: String,
    /// Short role description, if known.
    #[serde(default)]
    pub role: Option<String>,
    /// Sibling hub instance slug when `kind` is [`RoomSeatKind::SiblingA2a`].
    #[serde(default)]
    pub instance: Option<String>,
    /// Whether the seat is currently running / reachable.
    pub online: bool,
}

/// Response for `GET /api/hub/rooms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomListResponse {
    /// All active and archived rooms sorted by `updated_at` descending.
    pub rooms: Vec<RoomSummary>,
    /// Whether the hub's `SystemOne` decision model is configured and healthy.
    pub decision_model_active: bool,
}

/// Response for `GET /api/hub/rooms/candidates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomCandidatesResponse {
    /// Running local agents and online sibling/external A2A agents.
    pub candidates: Vec<RoomRosterCandidate>,
}

/// Request body for `POST /api/hub/rooms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateRoomRequest {
    /// Room display name (trimmed, 1..=120 chars).
    pub name: String,
    /// Optional topic/agenda.
    #[serde(default)]
    pub topic: Option<String>,
    /// Initial roster seat IDs (at least 1 required).
    pub seat_ids: Vec<String>,
    /// Whether to pin the room in the left rail.
    #[serde(default)]
    pub pinned: bool,
}

/// Request body for `PATCH /api/hub/rooms/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PatchRoomRequest {
    /// Updated room title.
    #[serde(default)]
    #[ts(optional)]
    pub name: Option<String>,
    /// Updated room topic.
    #[serde(default)]
    #[ts(optional)]
    pub topic: Option<String>,
    /// Updated pin flag.
    #[serde(default)]
    #[ts(optional)]
    pub pinned: Option<bool>,
    /// Seat IDs to add to the active roster.
    #[serde(default)]
    #[ts(optional)]
    pub add_seats: Option<Vec<String>>,
    /// Seat IDs to remove from the active roster.
    #[serde(default)]
    #[ts(optional)]
    pub remove_seats: Option<Vec<String>>,
}

/// Request body for `POST /api/hub/rooms/{id}/read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MarkRoomReadRequest {
    /// Highest sequence number observed by the client.
    #[ts(type = "number")]
    pub seq: u64,
}

/// Request body for `POST /api/hub/rooms/{id}/messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PostRoomMessageRequest {
    /// Owner message markdown text.
    pub content: String,
    /// Uploaded file IDs from `/api/hub/uploads` to attach.
    #[serde(default)]
    pub upload_ids: Vec<String>,
}

/// Response for `GET /api/hub/rooms/{id}`, `POST /api/hub/rooms`, and `POST /api/hub/rooms/{id}/archive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoomDetailResponse {
    /// Full room metadata and window state.
    pub room: RoomDetail,
    /// Transcript slice, oldest-to-newest.
    pub messages: Vec<RoomMessage>,
    /// Attached meeting minutes markdown if present.
    pub minutes_markdown: Option<String>,
    /// Whether the hub's `SystemOne` decision model is configured and healthy.
    pub decision_model_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_room_id_rejects_invalid_and_accepts_generated() {
        let generated = generate_room_id();
        assert!(
            validate_room_id(generated.as_str()).is_ok(),
            "generated room id {generated:?} should validate"
        );
        assert!(
            generated.as_str().starts_with("room_"),
            "generated room id {generated:?} should start with room_"
        );
        assert_eq!(
            generated.as_str().len(),
            19,
            "generated room id should be room_ + 14 base36 chars"
        );

        for valid in [
            "room_01jcv8k9m2p4q7",
            "room_abcdefgh",
            "room_0123456789abcdef0123456789abcdef",
        ] {
            assert!(
                validate_room_id(valid).is_ok(),
                "expected {valid:?} to be valid"
            );
        }

        for invalid in [
            "",
            "room_",
            "room_short",
            "room_1234567",
            "room_UPPERCASE",
            "room_01jcv8k9m2p4q7!",
            "../etc",
            "room_../etc0000",
            "other_01jcv8k9m2p4q7",
            "room_0123456789abcdef0123456789abcdef0",
        ] {
            assert!(
                validate_room_id(invalid).is_err(),
                "expected {invalid:?} to be rejected"
            );
        }
    }

    #[test]
    fn window_cap_for_roster_scales_as_ceil_three_halves() {
        let cases = [(1_usize, 2_u32), (2, 3), (3, 5), (4, 6), (5, 8), (7, 11)];
        for (roster_size, expected) in cases {
            assert_eq!(
                window_cap_for_roster(roster_size),
                expected,
                "window_cap_for_roster({roster_size}) should be {expected}"
            );
        }
    }

    #[test]
    fn room_record_json_roundtrip_matches_contract_shape() {
        let now = DateTime::parse_from_rfc3339("2026-11-14T13:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let record = RoomRecord {
            id: RoomId::parse("room_01jcv8k9m2p4q7").unwrap(),
            name: "Weekly Review".to_owned(),
            topic: Some("Priorities".to_owned()),
            status: RoomStatus::Active,
            pinned: true,
            created_at: now,
            updated_at: now,
            archived_at: None,
            roster: vec![RoomSeat {
                seat_id: "agent:scout".to_owned(),
                kind: RoomSeatKind::Local,
                display_name: "Scout".to_owned(),
                role: Some("Researcher".to_owned()),
                instance: None,
            }],
            historical_participants: vec!["agent:scout".to_owned()],
            last_seq: 4,
            last_read_seq: 2,
            last_seen_seq: BTreeMap::from([("agent:scout".to_owned(), 4)]),
            window: RoomWindowState::fresh(1),
            minutes: RoomMinutesMeta::default(),
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(
            json.get("id").and_then(serde_json::Value::as_str),
            Some("room_01jcv8k9m2p4q7"),
            "serialized room id must be flat string"
        );
        assert_eq!(
            json.get("status").and_then(serde_json::Value::as_str),
            Some("active"),
            "serialized status must be snake_case"
        );
        let roundtrip: RoomRecord = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip, record, "RoomRecord must roundtrip cleanly");
        assert_eq!(roundtrip.unread_count(), 2, "unread_count must be 4 - 2");
    }

    #[test]
    fn validate_room_name_trims_and_enforces_bounds() {
        assert_eq!(
            validate_room_name("  Architecture Sync  ").unwrap(),
            "Architecture Sync",
            "room name should be trimmed"
        );
        assert!(
            validate_room_name("   ").is_err(),
            "blank room name must be rejected"
        );
        let max_len = "a".repeat(MAX_ROOM_NAME_CHARS);
        assert!(
            validate_room_name(&max_len).is_ok(),
            "120-char room name should be accepted"
        );
        let too_long = "a".repeat(MAX_ROOM_NAME_CHARS + 1);
        assert!(
            validate_room_name(&too_long).is_err(),
            "121-char room name must be rejected"
        );
    }
}
