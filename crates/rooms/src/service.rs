//! Meeting rooms coordinator: lifecycle, turn/budget state machine,
//! archival/minutes generation, and live WebSocket event broadcast.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::Duration;

use chrono::Utc;
use thiserror::Error;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use residuum_a2a::A2aClientHub;
use residuum_config::WorkspaceLayout;
use residuum_inference::system_one::SystemOneService;
use residuum_workspace::inbox::mail::{Mailbox, MailboxDirectory};
use residuum_workspace::inbox::{self, InboxKind, InboxRef, NewInboxItem};
use residuum_workspace::team_files::{TeamWriteCoordinator, TeamWriter};

use crate::agent_view::RoomAgentView;
use crate::mentions::{parse_agent_baton_mentions, parse_owner_mentions};
use crate::participants::{
    A2aRoomDriver, LocalRoomParticipant, LocalRoomTurnRequest, RemoteRoomDriver,
    RemoteRoomTurnRequest, RoomTurnOutput, format_catchup_prompt,
};
use crate::selector::{SelectorOutcome, SpeakerSelector, SystemOneSpeakerSelector};
use crate::store::{NewRoomMessage, RoomStore, RoomStoreError};
use crate::types::{
    CreateRoomRequest, MAX_SEAT_TURNS_PER_WINDOW, PatchRoomRequest, RoomActiveTurn, RoomAttachment,
    RoomCandidatesResponse, RoomDetail, RoomDetailResponse, RoomId, RoomListResponse, RoomMessage,
    RoomMessageKind, RoomMinutesMeta, RoomRecord, RoomRosterCandidate, RoomSeat, RoomSeatKind,
    RoomStatus, RoomSummary, RoomValidationError, RoomWindowState, generate_room_id,
    validate_room_id, validate_room_name, window_cap_for_roster,
};

/// Bounded wait for `Stop` cancellation so `/stop` always returns within `1s`.
const STOP_CANCEL_WAIT: Duration = Duration::from_millis(800);

/// Maximum character count for a room slug in `team/minutes/YYYY-MM-DD-<slug>.md`.
const MAX_MINUTES_SLUG_LEN: usize = 60;

/// Errors returned by [`RoomService`] operations, structured for HTTP status
/// mapping in `residuum-hub`.
#[derive(Debug, Error)]
pub enum RoomServiceError {
    /// Request validation failed (`400 Bad Request`).
    #[error(transparent)]
    Validation(#[from] RoomValidationError),
    /// The requested room does not exist (`404 Not Found`).
    #[error("room '{0}' not found")]
    NotFound(String),
    /// Invalid request payload or state transition (`400 Bad Request`).
    #[error("{0}")]
    BadRequest(String),
    /// State conflict such as archived room or turn in flight (`409 Conflict`).
    #[error("{0}")]
    Conflict(String),
    /// Agent is not a participant in the requested room.
    #[error("{0}")]
    AccessDenied(String),
    /// Underlying store failure (`500 Internal Server Error`).
    #[error("room storage error: {0}")]
    Store(RoomStoreError),
    /// Unexpected internal error (`500 Internal Server Error`).
    #[error("{0}")]
    Internal(String),
}

impl From<RoomStoreError> for RoomServiceError {
    fn from(err: RoomStoreError) -> Self {
        match err {
            RoomStoreError::InvalidRoomId(v) => Self::Validation(v),
            RoomStoreError::NotFound(id) => Self::NotFound(id),
            other @ (RoomStoreError::AlreadyExists(_)
            | RoomStoreError::Io(_)
            | RoomStoreError::Json(_)
            | RoomStoreError::Write(_)) => Self::Store(other),
        }
    }
}

/// Live hub broadcast events emitted by [`RoomService`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomEvent {
    /// A room's metadata, roster, pin, read state, window, or status changed.
    Updated(RoomSummary),
    /// A new message was appended to `transcript.jsonl`.
    Message {
        /// Room ID.
        room_id: RoomId,
        /// Newly appended message.
        message: RoomMessage,
        /// Updated unread message count for the room.
        unread_count: u64,
    },
    /// Active turn, tool status, queue, or window budget changed.
    TurnState {
        /// Room ID.
        room_id: RoomId,
        /// Active speaker turn, or `None` when idle.
        active_turn: Option<RoomActiveTurn>,
        /// Seat IDs currently queued to speak.
        queued_seat_ids: Vec<String>,
        /// Whether the room paused at `window_cap` while another turn waited.
        paused_at_cap: bool,
        /// Completed turns in the current window.
        window_used: u32,
        /// Cap for the current window.
        window_cap: u32,
    },
    /// A room was permanently deleted.
    Deleted {
        /// Deleted room ID.
        room_id: RoomId,
    },
}

/// Registration descriptor for a running local hub agent.
#[derive(Clone)]
pub struct LocalRoomAgentInfo {
    /// Folder name (e.g. `"scout"`), forming seat ID `"agent:<name>"`.
    pub name: String,
    /// Display name (e.g. `"Scout"`).
    pub display_name: String,
    /// Optional one-line role description.
    pub role: Option<String>,
    /// Turn/session handler for this agent.
    pub participant: Arc<dyn LocalRoomParticipant>,
}

struct ActiveRun {
    run_id: u64,
    cancel_token: CancellationToken,
    active_turn: Arc<StdMutex<Option<RoomActiveTurn>>>,
    join_handle: JoinHandle<()>,
}

enum StepAction {
    RunSeat(RoomSeat),
    ConsultSelector {
        room_name: String,
        room_topic: Option<String>,
        eligible: Vec<RoomSeat>,
        at_cap: bool,
    },
    StopIdle,
    StopPausedAtCap,
}

/// Coordinator service owning room lifecycle, turn loops, and archive/minutes.
pub struct RoomService {
    store: RoomStore,
    snapshots: StdRwLock<BTreeMap<String, RoomRecord>>,
    selector: Arc<dyn SpeakerSelector>,
    remote: Arc<dyn RemoteRoomDriver>,
    local_agents: StdRwLock<BTreeMap<String, LocalRoomAgentInfo>>,
    active_runs: Mutex<HashMap<String, ActiveRun>>,
    room_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    next_run_id: AtomicU64,
    team: TeamWriteCoordinator,
    mailboxes: Arc<dyn MailboxDirectory>,
    events_tx: broadcast::Sender<RoomEvent>,
}

impl RoomService {
    /// Create a [`RoomService`] wired to the hub's real System 1 and A2A drivers.
    pub async fn new(
        store: RoomStore,
        system_one: Arc<SystemOneService>,
        a2a_hub: Arc<A2aClientHub>,
        team: TeamWriteCoordinator,
        mailboxes: Arc<dyn MailboxDirectory>,
    ) -> Arc<Self> {
        Self::with_drivers(
            store,
            Arc::new(SystemOneSpeakerSelector::new(system_one)),
            Arc::new(A2aRoomDriver::new(a2a_hub)),
            team,
            mailboxes,
        )
        .await
    }

    /// Create an empty [`RoomService`] synchronously for test harnesses.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn new_empty_sync(
        store: RoomStore,
        system_one: Arc<SystemOneService>,
        a2a_hub: Arc<A2aClientHub>,
        team: TeamWriteCoordinator,
        mailboxes: Arc<dyn MailboxDirectory>,
    ) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            store,
            snapshots: StdRwLock::new(BTreeMap::new()),
            selector: Arc::new(SystemOneSpeakerSelector::new(system_one)),
            remote: Arc::new(A2aRoomDriver::new(a2a_hub)),
            local_agents: StdRwLock::new(BTreeMap::new()),
            active_runs: Mutex::new(HashMap::new()),
            room_locks: Mutex::new(HashMap::new()),
            next_run_id: AtomicU64::new(1),
            team,
            mailboxes,
            events_tx,
        })
    }

    /// Create a [`RoomService`] with custom [`SpeakerSelector`] and
    /// [`RemoteRoomDriver`] implementations (primarily for testing).
    pub async fn with_drivers(
        store: RoomStore,
        selector: Arc<dyn SpeakerSelector>,
        remote: Arc<dyn RemoteRoomDriver>,
        team: TeamWriteCoordinator,
        mailboxes: Arc<dyn MailboxDirectory>,
    ) -> Arc<Self> {
        let records = store.list_records().await;
        let initial_snapshots: BTreeMap<String, RoomRecord> =
            records.into_iter().map(|r| (r.id.to_string(), r)).collect();
        let (events_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            store,
            snapshots: StdRwLock::new(initial_snapshots),
            selector,
            remote,
            local_agents: StdRwLock::new(BTreeMap::new()),
            active_runs: Mutex::new(HashMap::new()),
            room_locks: Mutex::new(HashMap::new()),
            next_run_id: AtomicU64::new(1),
            team,
            mailboxes,
            events_tx,
        })
    }

    /// Borrow the underlying [`RoomStore`].
    #[must_use]
    pub fn store(&self) -> &RoomStore {
        &self.store
    }

    async fn room_lock(&self, room_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.room_locks.lock().await;
        Arc::clone(
            locks
                .entry(room_id.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    pub(crate) fn snapshot_records(&self) -> Vec<RoomRecord> {
        self.snapshots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    fn sync_snapshot(&self, record: &RoomRecord) {
        self.snapshots
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(record.id.to_string(), record.clone());
    }

    fn remove_snapshot(&self, room_id: &str) {
        self.snapshots
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(room_id);
    }

    fn emit(&self, event: RoomEvent) {
        self.events_tx.send(event).ok();
    }

    fn emit_record_updated(&self, record: &RoomRecord) {
        self.emit(RoomEvent::Updated(RoomSummary::from_record(record)));
    }

    fn emit_turn_state(&self, record: &RoomRecord, active_turn: Option<RoomActiveTurn>) {
        self.emit(RoomEvent::TurnState {
            room_id: record.id.clone(),
            active_turn,
            queued_seat_ids: record.window.queued_seats.clone(),
            paused_at_cap: record.window.paused_at_cap,
            window_used: record.window.window_used,
            window_cap: record.window.window_cap,
        });
    }

    /// Register a running local agent so it appears as a candidate and can
    /// take room turns.
    pub fn register_local_agent(&self, info: LocalRoomAgentInfo) {
        self.local_agents
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(info.name.clone(), info);
    }

    /// Remove a stopping local agent from the active registry.
    pub fn unregister_local_agent(&self, name: &str) {
        self.local_agents
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
    }

    fn get_local_agent(&self, name: &str) -> Option<LocalRoomAgentInfo> {
        self.local_agents
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    fn all_local_agents(&self) -> Vec<LocalRoomAgentInfo> {
        self.local_agents
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Subscribe to live room events.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<RoomEvent> {
        self.events_tx.subscribe()
    }

    /// Return a participant-scoped [`RoomAgentView`] for `agent_name`.
    #[must_use]
    pub fn agent_view(self: &Arc<Self>, agent_name: impl Into<String>) -> RoomAgentView {
        RoomAgentView::new(agent_name, Arc::clone(self))
    }

    /// List all active and archived rooms sorted by `updated_at` descending.
    pub async fn list_rooms(&self) -> RoomListResponse {
        RoomListResponse {
            rooms: self.store.list_summaries().await,
            decision_model_active: self.selector.is_available(),
        }
    }

    /// List eligible local and connected A2A candidate seats.
    pub async fn list_candidates(&self) -> RoomCandidatesResponse {
        let mut candidates: Vec<RoomRosterCandidate> = self
            .all_local_agents()
            .into_iter()
            .map(|info| RoomRosterCandidate {
                seat_id: format!("agent:{}", info.name),
                kind: RoomSeatKind::Local,
                display_name: info.display_name,
                role: info.role,
                instance: None,
                online: true,
            })
            .collect();
        candidates.extend(self.remote.candidates().await);
        RoomCandidatesResponse { candidates }
    }

    async fn resolve_seat_for_join(&self, seat_id: &str) -> Result<RoomSeat, RoomServiceError> {
        if let Some(local_name) = seat_id.strip_prefix("agent:") {
            let info = self.get_local_agent(local_name).ok_or_else(|| {
                RoomServiceError::BadRequest(format!(
                    "local agent '{seat_id}' is not running or does not exist"
                ))
            })?;
            return Ok(RoomSeat {
                seat_id: seat_id.to_owned(),
                kind: RoomSeatKind::Local,
                display_name: info.display_name,
                role: info.role,
                instance: None,
            });
        }
        if seat_id.starts_with("a2a:") {
            let cand = self.remote.resolve_seat(seat_id).await.ok_or_else(|| {
                RoomServiceError::BadRequest(format!(
                    "remote A2A agent '{seat_id}' is not registered"
                ))
            })?;
            if !cand.online {
                return Err(RoomServiceError::BadRequest(format!(
                    "remote A2A agent '{seat_id}' is currently offline"
                )));
            }
            return Ok(RoomSeat {
                seat_id: cand.seat_id,
                kind: cand.kind,
                display_name: cand.display_name,
                role: cand.role,
                instance: cand.instance,
            });
        }
        Err(RoomServiceError::BadRequest(format!(
            "invalid seat id '{seat_id}': expected 'agent:<name>' or 'a2a:<name>'"
        )))
    }

    async fn resolve_unique_seats(
        &self,
        seat_ids: &[String],
    ) -> Result<Vec<RoomSeat>, RoomServiceError> {
        let mut seats = Vec::new();
        for seat_id in seat_ids {
            if seats.iter().any(|s: &RoomSeat| s.seat_id == *seat_id) {
                continue;
            }
            seats.push(self.resolve_seat_for_join(seat_id).await?);
        }
        Ok(seats)
    }

    /// Create a new active meeting room.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::Validation`] or [`RoomServiceError::BadRequest`]
    /// on invalid name, empty roster, or unknown/offline seats.
    pub async fn create_room(
        &self,
        req: CreateRoomRequest,
    ) -> Result<RoomDetailResponse, RoomServiceError> {
        let name = validate_room_name(&req.name)?;
        let roster = self.resolve_unique_seats(&req.seat_ids).await?;
        if roster.is_empty() {
            return Err(RoomServiceError::BadRequest(
                "at least one participant seat is required".to_owned(),
            ));
        }
        let topic = normalize_optional_topic(req.topic);
        let now = Utc::now();
        let historical_participants = roster.iter().map(|s| s.seat_id.clone()).collect();
        let window = RoomWindowState::fresh(roster.len());

        let record = RoomRecord {
            id: generate_room_id(),
            name,
            topic,
            status: RoomStatus::Active,
            pinned: req.pinned,
            created_at: now,
            updated_at: now,
            archived_at: None,
            roster,
            historical_participants,
            last_seq: 0,
            last_read_seq: 0,
            last_seen_seq: BTreeMap::new(),
            window,
            minutes: RoomMinutesMeta::default(),
        };
        let created = self.store.create_room(record).await?;
        self.sync_snapshot(&created);
        self.emit_record_updated(&created);
        let decision_model_active = self.detail_decision_active(&created);
        Ok(RoomDetailResponse {
            room: RoomDetail::from_record(&created, None),
            messages: Vec::new(),
            minutes_markdown: None,
            decision_model_active,
        })
    }

    async fn current_active_turn(&self, room_id: &str) -> Option<RoomActiveTurn> {
        let runs = self.active_runs.lock().await;
        runs.get(room_id).and_then(|run| {
            run.active_turn
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        })
    }

    fn detail_decision_active(&self, record: &RoomRecord) -> bool {
        self.selector.is_available() && !record.window.decision_model_fallback
    }

    async fn clear_fallback_if_available(&self, room_id: &str, record: &mut RoomRecord) {
        if self.selector.is_available()
            && record.window.decision_model_fallback
            && let Ok((updated, ())) = self
                .store
                .update_record(room_id, |rec| {
                    rec.window.decision_model_fallback = false;
                })
                .await
        {
            self.sync_snapshot(&updated);
            *record = updated;
        }
    }

    /// Fetch room metadata, transcript slice, and attached minutes.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::Validation`] or [`RoomServiceError::NotFound`].
    pub async fn get_room(
        &self,
        room_id: &str,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<RoomDetailResponse, RoomServiceError> {
        let mut record = self.store.get_record(room_id).await?;
        self.clear_fallback_if_available(room_id, &mut record).await;
        let messages = self
            .store
            .read_messages(room_id, limit.min(200), before_seq)
            .await?;
        let minutes_markdown = self.store.read_minutes(room_id).await?;
        let active_turn = self.current_active_turn(room_id).await;
        let decision_model_active = self.detail_decision_active(&record);
        Ok(RoomDetailResponse {
            room: RoomDetail::from_record(&record, active_turn),
            messages,
            minutes_markdown,
            decision_model_active,
        })
    }

    /// Update room metadata, pin state, or active roster.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::BadRequest`] on archived rooms, invalid names,
    /// unknown/offline added seats, or removing all remaining seats.
    pub async fn patch_room(
        &self,
        room_id: &str,
        req: PatchRoomRequest,
    ) -> Result<RoomDetail, RoomServiceError> {
        validate_room_id(room_id)?;
        let added = match &req.add_seats {
            Some(ids) => self.resolve_unique_seats(ids).await?,
            None => Vec::new(),
        };
        let validated_name = req.name.as_deref().map(validate_room_name).transpose()?;
        let room_mutex = self.room_lock(room_id).await;
        let _room_guard = room_mutex.lock().await;

        let (updated, ()) = self
            .store
            .try_update_record(room_id, |rec| {
                if rec.status == RoomStatus::Archived {
                    return Err(RoomServiceError::BadRequest(
                        "archived room cannot be edited; reopen it first".to_owned(),
                    ));
                }
                if let Some(name) = validated_name {
                    rec.name = name;
                }
                if let Some(topic) = &req.topic {
                    let trimmed = topic.trim();
                    rec.topic = (!trimmed.is_empty()).then(|| trimmed.to_owned());
                }
                if let Some(pinned) = req.pinned {
                    rec.pinned = pinned;
                }
                for seat in added {
                    if !rec.roster.iter().any(|s| s.seat_id == seat.seat_id) {
                        rec.roster.push(seat.clone());
                    }
                    if !rec.historical_participants.contains(&seat.seat_id) {
                        rec.historical_participants.push(seat.seat_id);
                    }
                }
                if let Some(remove_ids) = &req.remove_seats {
                    rec.roster
                        .retain(|seat| !remove_ids.iter().any(|id| id == &seat.seat_id));
                    rec.window
                        .queued_seats
                        .retain(|id| !remove_ids.iter().any(|rem| rem == id));
                    if rec.roster.is_empty() {
                        return Err(RoomServiceError::BadRequest(
                            "cannot remove all participants from an active room".to_owned(),
                        ));
                    }
                }
                rec.updated_at = Utc::now();
                Ok(())
            })
            .await?;

        self.sync_snapshot(&updated);
        self.emit_record_updated(&updated);
        let active_turn = self.current_active_turn(room_id).await;
        Ok(RoomDetail::from_record(&updated, active_turn))
    }

    /// Advance the owner's read cursor (`last_read_seq`).
    ///
    /// # Errors
    /// Returns [`RoomServiceError::Validation`] or [`RoomServiceError::NotFound`].
    pub async fn mark_read(&self, room_id: &str, seq: u64) -> Result<(), RoomServiceError> {
        let updated = self.store.mark_read(room_id, seq).await?;
        self.sync_snapshot(&updated);
        self.emit_record_updated(&updated);
        Ok(())
    }

    /// Post an owner message, open a fresh `ceil(1.5 * R)` turn window, and
    /// spawn the sequential turn loop.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::BadRequest`] on empty content + attachments,
    /// or [`RoomServiceError::Conflict`] if archived or a turn is in flight.
    pub async fn post_owner_message(
        self: &Arc<Self>,
        room_id: &str,
        content: String,
        attachments: Vec<RoomAttachment>,
    ) -> Result<RoomMessage, RoomServiceError> {
        validate_room_id(room_id)?;
        if content.trim().is_empty() && attachments.is_empty() {
            return Err(RoomServiceError::BadRequest(
                "message content or attachments required".to_owned(),
            ));
        }

        // Fast pre-lock check so posts fail immediately with 409 while a slow
        // archive_room minutes generation holds the per-room lock.
        let pre = self.store.get_record(room_id).await?;
        if pre.status == RoomStatus::Archived {
            return Err(RoomServiceError::Conflict(
                "cannot post to an archived room; reopen it first".to_owned(),
            ));
        }

        let room_mutex = self.room_lock(room_id).await;
        let _room_guard = room_mutex.lock().await;

        if self.active_runs.lock().await.contains_key(room_id) {
            return Err(RoomServiceError::Conflict(
                "a turn is currently in progress; stop the room or wait for it to finish"
                    .to_owned(),
            ));
        }

        let current = self.store.get_record(room_id).await?;
        if current.status == RoomStatus::Archived {
            return Err(RoomServiceError::Conflict(
                "cannot post to an archived room; reopen it first".to_owned(),
            ));
        }

        let (_, message) = self
            .store
            .append_message(
                room_id,
                NewRoomMessage {
                    kind: RoomMessageKind::Owner,
                    seat_id: None,
                    author_name: "Owner".to_owned(),
                    seat_kind: None,
                    content,
                    attachments,
                    timestamp: None,
                },
            )
            .await?;

        let (updated, ()) = self
            .store
            .update_record(room_id, |rec| {
                let mut window = RoomWindowState::fresh(rec.roster.len());
                window.queued_seats = parse_owner_mentions(&message.content, &rec.roster);
                rec.window = window;
            })
            .await?;

        self.sync_snapshot(&updated);
        self.emit(RoomEvent::Message {
            room_id: updated.id.clone(),
            message: message.clone(),
            unread_count: updated.unread_count(),
        });
        self.emit_record_updated(&updated);
        self.emit_turn_state(&updated, None);

        {
            let mut runs = self.active_runs.lock().await;
            self.spawn_turn_loop_locked(&mut runs, room_id.to_owned(), None);
        }
        Ok(message)
    }

    /// Grant another `ceil(1.5 * R)` turn window when paused at cap.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::Conflict`] unless active and `paused_at_cap`,
    /// or if a turn is currently in flight.
    pub async fn continue_window(
        self: &Arc<Self>,
        room_id: &str,
    ) -> Result<RoomDetail, RoomServiceError> {
        validate_room_id(room_id)?;
        // Fast pre-lock check so continues fail immediately with 409 while
        // archive_room holds the per-room lock during minutes generation.
        let pre = self.store.get_record(room_id).await?;
        if pre.status != RoomStatus::Active {
            return Err(RoomServiceError::Conflict(
                "cannot continue an archived room".to_owned(),
            ));
        }

        let room_mutex = self.room_lock(room_id).await;
        let _room_guard = room_mutex.lock().await;

        if self.active_runs.lock().await.contains_key(room_id) {
            return Err(RoomServiceError::Conflict(
                "a turn is already running in this room".to_owned(),
            ));
        }

        let (updated, ()) = self
            .store
            .try_update_record(room_id, |rec| {
                if rec.status != RoomStatus::Active {
                    return Err(RoomServiceError::Conflict(
                        "cannot continue an archived room".to_owned(),
                    ));
                }
                if !rec.window.paused_at_cap {
                    return Err(RoomServiceError::Conflict(
                        "room is not paused at a turn budget cap".to_owned(),
                    ));
                }
                rec.window.roster_size_at_start = rec.roster.len();
                rec.window.window_cap = window_cap_for_roster(rec.roster.len());
                rec.window.window_used = 0;
                rec.window.per_seat_used.clear();
                rec.window.paused_at_cap = false;
                Ok(())
            })
            .await?;

        let last_speaker = self.read_last_speaker_seat(room_id).await;
        self.sync_snapshot(&updated);
        self.emit_record_updated(&updated);
        self.emit_turn_state(&updated, None);
        {
            let mut runs = self.active_runs.lock().await;
            self.spawn_turn_loop_locked(&mut runs, room_id.to_owned(), last_speaker);
        }
        Ok(RoomDetail::from_record(&updated, None))
    }

    async fn read_last_speaker_seat(&self, room_id: &str) -> Option<String> {
        let tail = self.store.read_messages(room_id, 1, None).await.ok()?;
        let last = tail.last()?;
        (last.kind == RoomMessageKind::Agent)
            .then(|| last.seat_id.clone())
            .flatten()
    }

    /// Cancel any active turn and clear queued seats within 1 second.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::Validation`] or [`RoomServiceError::NotFound`].
    pub async fn stop_room(&self, room_id: &str) -> Result<RoomDetail, RoomServiceError> {
        validate_room_id(room_id)?;
        let room_mutex = self.room_lock(room_id).await;
        let _room_guard = room_mutex.lock().await;
        let _ = self.store.get_record(room_id).await?;
        let updated = self.stop_room_internal(room_id).await?;
        Ok(RoomDetail::from_record(&updated, None))
    }

    async fn stop_room_internal(&self, room_id: &str) -> Result<RoomRecord, RoomServiceError> {
        let taken = self.active_runs.lock().await.remove(room_id);
        if let Some(run) = taken {
            run.cancel_token.cancel();
            let active_turn = run
                .active_turn
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let mut join_handle = run.join_handle;
            let cancel_remote = async {
                if let Some(turn) = active_turn {
                    self.cancel_seat_turn(room_id, &turn).await;
                }
            };
            let wait_join = async {
                (&mut join_handle).await.ok();
            };
            if tokio::time::timeout(STOP_CANCEL_WAIT, async {
                tokio::join!(cancel_remote, wait_join);
            })
            .await
            .is_err()
            {
                tracing::warn!(room_id, "room stop timed out at 800ms; aborting turn task");
                join_handle.abort();
            }
        }

        let current = self.store.get_record(room_id).await?;
        let updated = if !current.window.queued_seats.is_empty() || current.window.paused_at_cap {
            let (rec, ()) = self
                .store
                .update_record(room_id, |rec| {
                    rec.window.queued_seats.clear();
                    rec.window.paused_at_cap = false;
                })
                .await?;
            rec
        } else {
            current
        };
        self.sync_snapshot(&updated);
        self.emit_turn_state(&updated, None);
        self.emit_record_updated(&updated);
        Ok(updated)
    }

    async fn cancel_seat_turn(&self, room_id: &str, turn: &RoomActiveTurn) {
        match turn.seat_kind {
            RoomSeatKind::Local => {
                if let Some(local_name) = turn.seat_id.strip_prefix("agent:")
                    && let Some(info) = self.get_local_agent(local_name)
                    && let Err(err) = info.participant.cancel_room_turn(room_id).await
                {
                    tracing::warn!(room_id, seat_id = %turn.seat_id, error = %err, "failed to cancel local room turn");
                }
            }
            RoomSeatKind::SiblingA2a | RoomSeatKind::ExternalA2a => {
                if let Err(err) = self.remote.cancel_remote_turn(room_id, &turn.seat_id).await {
                    tracing::warn!(room_id, seat_id = %turn.seat_id, error = %err, "failed to cancel remote room turn");
                }
            }
        }
    }

    /// Lock a room read-only, close participating sessions, and generate
    /// minutes + inbox notification when new messages exist.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::Validation`], [`RoomServiceError::NotFound`],
    /// or storage/minutes write errors.
    pub async fn archive_room(
        &self,
        room_id: &str,
    ) -> Result<RoomDetailResponse, RoomServiceError> {
        validate_room_id(room_id)?;
        let room_mutex = self.room_lock(room_id).await;
        let _room_guard = room_mutex.lock().await;

        let _ = self.store.get_record(room_id).await?;
        let stopped = self.stop_room_internal(room_id).await?;
        let now = Utc::now();
        let should_generate =
            stopped.last_seq > 0 && stopped.last_seq > stopped.minutes.archived_at_seq;

        let (mut archived, ()) = self
            .store
            .update_record(room_id, |rec| {
                rec.status = RoomStatus::Archived;
                if rec.archived_at.is_none() {
                    rec.archived_at = Some(now);
                }
                rec.updated_at = now;
                rec.window.queued_seats.clear();
                rec.window.paused_at_cap = false;
            })
            .await?;
        self.sync_snapshot(&archived);
        self.emit_record_updated(&archived);

        self.close_participating_sessions(&archived).await;

        if !should_generate {
            self.clear_fallback_if_available(room_id, &mut archived)
                .await;
            let minutes_markdown = self.store.read_minutes(room_id).await?;
            let decision_model_active = self.detail_decision_active(&archived);
            return Ok(RoomDetailResponse {
                room: RoomDetail::from_record(&archived, None),
                messages: self.store.read_all_messages(room_id).await?,
                minutes_markdown,
                decision_model_active,
            });
        }

        self.finalize_archive_minutes(&archived, now).await
    }

    async fn finalize_archive_minutes(
        &self,
        archived: &RoomRecord,
        started_at: chrono::DateTime<Utc>,
    ) -> Result<RoomDetailResponse, RoomServiceError> {
        let room_id = archived.id.as_str();
        let messages = self.store.read_all_messages(room_id).await?;
        let (markdown, writer) = self.build_minutes_markdown(archived, &messages).await;
        self.store.write_minutes(room_id, &markdown).await?;

        let shared_path = self.resolve_shared_minutes_path(archived, started_at);
        self.commit_shared_minutes(&shared_path, &writer, &markdown)
            .await?;
        let inbox_ref = self
            .deliver_minutes_inbox(archived, &shared_path, &markdown)
            .await;

        let finished_at = Utc::now();
        let (mut updated, ()) = self
            .store
            .update_record(room_id, |rec| {
                rec.updated_at = finished_at;
                rec.minutes = RoomMinutesMeta {
                    shared_path: Some(shared_path),
                    archived_at_seq: archived.last_seq,
                    inbox_ref: inbox_ref.or_else(|| rec.minutes.inbox_ref.clone()),
                    updated_at: Some(finished_at),
                };
            })
            .await?;
        if self.selector.is_available() && updated.window.decision_model_fallback {
            let (cleared, ()) = self
                .store
                .update_record(room_id, |rec| {
                    rec.window.decision_model_fallback = false;
                })
                .await?;
            updated = cleared;
        }
        self.sync_snapshot(&updated);
        self.emit_record_updated(&updated);
        let decision_model_active = self.detail_decision_active(&updated);
        Ok(RoomDetailResponse {
            room: RoomDetail::from_record(&updated, None),
            messages,
            minutes_markdown: Some(markdown),
            decision_model_active,
        })
    }

    async fn close_participating_sessions(&self, record: &RoomRecord) {
        let room_id = record.id.as_str();
        for seat_id in &record.historical_participants {
            if let Some(local_name) = seat_id.strip_prefix("agent:") {
                if let Some(info) = self.get_local_agent(local_name)
                    && let Err(err) = info.participant.close_room_session(room_id).await
                {
                    tracing::warn!(room_id, seat_id = %seat_id, error = %err, "failed to close local room session");
                }
            } else if self.is_sibling_seat(record, seat_id).await
                && let Err(err) = self.remote.close_remote_session(room_id, seat_id).await
            {
                tracing::warn!(room_id, seat_id = %seat_id, error = %err, "failed to close sibling room session");
            }
        }
    }

    async fn is_sibling_seat(&self, record: &RoomRecord, seat_id: &str) -> bool {
        if let Some(seat) = record.roster.iter().find(|s| s.seat_id == seat_id) {
            return seat.kind == RoomSeatKind::SiblingA2a;
        }
        if let Some(cand) = self.remote.resolve_seat(seat_id).await {
            return cand.kind == RoomSeatKind::SiblingA2a;
        }
        seat_id
            .strip_prefix("a2a:")
            .is_some_and(|rest| rest.contains('/'))
    }

    async fn build_minutes_markdown(
        &self,
        record: &RoomRecord,
        messages: &[RoomMessage],
    ) -> (String, TeamWriter) {
        let local_candidate = record
            .roster
            .iter()
            .filter_map(|s| s.seat_id.strip_prefix("agent:"))
            .find_map(|name| self.get_local_agent(name))
            .or_else(|| self.all_local_agents().into_iter().next());

        if let Some(agent) = local_candidate {
            let prompt = format_minutes_prompt(record, messages);
            match agent.participant.generate_minutes(prompt).await {
                Ok(text) if !text.trim().is_empty() => {
                    return (text, TeamWriter::Agent(agent.name));
                }
                Ok(_) => {
                    tracing::warn!(room_id = %record.id, "agent returned empty minutes; using fallback template");
                }
                Err(err) => {
                    tracing::warn!(room_id = %record.id, error = %err, "agent minutes generation failed; using fallback template");
                }
            }
        }
        (
            fallback_minutes_markdown(record, messages),
            TeamWriter::User,
        )
    }

    fn resolve_shared_minutes_path(
        &self,
        record: &RoomRecord,
        now: chrono::DateTime<Utc>,
    ) -> String {
        if let Some(existing) = &record.minutes.shared_path {
            return existing.clone();
        }
        let date = now.format("%Y-%m-%d");
        let slug = slugify_room_name(&record.name, record.id.as_str());
        let base_rel = format!("minutes/{date}-{slug}.md");
        if !self.team.root().join(&base_rel).exists() {
            return format!("team/{base_rel}");
        }
        for n in 2..1000_u32 {
            let candidate_rel = format!("minutes/{date}-{slug}-{n}.md");
            if !self.team.root().join(&candidate_rel).exists() {
                return format!("team/{candidate_rel}");
            }
        }
        format!("team/minutes/{date}-{}-{}.md", slug, record.id)
    }

    async fn commit_shared_minutes(
        &self,
        shared_path: &str,
        writer: &TeamWriter,
        markdown: &str,
    ) -> Result<(), RoomServiceError> {
        let rel = shared_path.strip_prefix("team/").unwrap_or(shared_path);
        let full_path = self.team.root().join(rel);
        let guard = self.team.lock(&full_path).await;
        guard
            .commit(writer, markdown.as_bytes())
            .await
            .map_err(|err| {
                RoomServiceError::Internal(format!("failed to write {shared_path}: {err}"))
            })?;
        Ok(())
    }

    fn pick_target_mailbox(&self, record: &RoomRecord) -> Option<Mailbox> {
        record
            .roster
            .iter()
            .filter_map(|seat| seat.seat_id.strip_prefix("agent:"))
            .find_map(|name| self.mailboxes.mailbox(name))
            .or_else(|| self.mailboxes.mailboxes().into_iter().next())
    }

    async fn deliver_minutes_inbox(
        &self,
        record: &RoomRecord,
        shared_path: &str,
        markdown: &str,
    ) -> Option<String> {
        let mailbox = self.pick_target_mailbox(record)?;
        let layout = WorkspaceLayout::new(&mailbox.dir);
        let title = format!("Meeting minutes: {}", record.name);
        let body = format!("{markdown}\n\n---\nSaved to `{shared_path}`.");
        let source = format!("room:{}", record.id);

        let (filename, _) = match inbox::add_item(
            &layout.user_inbox_dir(),
            NewInboxItem {
                title: &title,
                body: &body,
                source: &source,
                in_reply_to: None,
                attachments: &[],
            },
            mailbox.timezone,
        )
        .await
        {
            Ok(res) => res,
            Err(err) => {
                tracing::warn!(room_id = %record.id, agent = %mailbox.agent, error = %err, "failed to write room minutes inbox item");
                return None;
            }
        };

        let item_id = filename.strip_suffix(".json").unwrap_or(&filename);
        if let Some(info) = self.get_local_agent(&mailbox.agent) {
            info.participant.announce_user_inbox(item_id).await;
        }
        InboxRef::new(&mailbox.agent, InboxKind::User, item_id)
            .ok()
            .map(|r| r.to_string())
            .or_else(|| Some(format!("{}/user/{item_id}", mailbox.agent)))
    }

    /// Reopen an archived room for further messages.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::Validation`] or [`RoomServiceError::NotFound`].
    pub async fn reopen_room(&self, room_id: &str) -> Result<RoomDetail, RoomServiceError> {
        validate_room_id(room_id)?;
        let room_mutex = self.room_lock(room_id).await;
        let _room_guard = room_mutex.lock().await;
        let now = Utc::now();
        let (updated, ()) = self
            .store
            .update_record(room_id, |rec| {
                rec.status = RoomStatus::Active;
                rec.archived_at = None;
                rec.updated_at = now;
                rec.window.queued_seats.clear();
                rec.window.paused_at_cap = false;
            })
            .await?;
        self.sync_snapshot(&updated);
        self.emit_record_updated(&updated);
        Ok(RoomDetail::from_record(&updated, None))
    }

    /// Permanently delete a room, stopping any active turn first.
    ///
    /// # Errors
    /// Returns [`RoomServiceError::Validation`] or [`RoomServiceError::NotFound`].
    pub async fn delete_room(&self, room_id: &str) -> Result<(), RoomServiceError> {
        validate_room_id(room_id)?;
        let room_mutex = self.room_lock(room_id).await;
        let _room_guard = room_mutex.lock().await;
        let record = self.store.get_record(room_id).await?;
        let _ = self.stop_room_internal(room_id).await?;
        self.close_participating_sessions(&record).await;
        self.store.delete_room(room_id).await?;
        self.remove_snapshot(room_id);
        self.room_locks.lock().await.remove(room_id);
        self.emit(RoomEvent::Deleted { room_id: record.id });
        Ok(())
    }

    fn spawn_turn_loop_locked(
        self: &Arc<Self>,
        runs: &mut HashMap<String, ActiveRun>,
        room_id: String,
        initial_last_speaker: Option<String>,
    ) {
        let run_id = self.next_run_id.fetch_add(1, Ordering::Relaxed);
        let cancel_token = CancellationToken::new();
        let active_turn = Arc::new(StdMutex::new(None));
        let svc = Arc::clone(self);
        let token_for_task = cancel_token.clone();
        let turn_slot = Arc::clone(&active_turn);
        let room_key = room_id.clone();

        let join_handle = residuum_core::util::spawn_in_span(async move {
            svc.run_turn_loop(
                &room_key,
                run_id,
                token_for_task,
                turn_slot,
                initial_last_speaker,
            )
            .await;
        });

        runs.insert(
            room_id,
            ActiveRun {
                run_id,
                cancel_token,
                active_turn,
                join_handle,
            },
        );
    }

    async fn run_turn_loop(
        self: Arc<Self>,
        room_id: &str,
        run_id: u64,
        cancel_token: CancellationToken,
        active_turn_slot: Arc<StdMutex<Option<RoomActiveTurn>>>,
        mut last_speaker_seat: Option<String>,
    ) {
        loop {
            if cancel_token.is_cancelled() {
                break;
            }
            let step = match self
                .resolve_next_step(room_id, last_speaker_seat.as_deref())
                .await
            {
                Ok(step) => step,
                Err(err) => {
                    tracing::warn!(room_id, error = %err, "room turn step resolution failed");
                    break;
                }
            };

            let seat = match step {
                StepAction::StopIdle | StepAction::StopPausedAtCap => break,
                StepAction::RunSeat(seat) => seat,
                StepAction::ConsultSelector {
                    room_name,
                    room_topic,
                    eligible,
                    at_cap,
                } => {
                    let Some(picked) = self
                        .consult_selector_step(
                            room_id,
                            &room_name,
                            room_topic.as_deref(),
                            &eligible,
                            at_cap,
                            &cancel_token,
                        )
                        .await
                    else {
                        break;
                    };
                    picked
                }
            };

            match self
                .execute_seat_turn(room_id, &seat, &cancel_token, &active_turn_slot)
                .await
            {
                TurnStepOutcome::Completed { speaker_seat_id } => {
                    last_speaker_seat = Some(speaker_seat_id);
                }
                TurnStepOutcome::Cancelled | TurnStepOutcome::HaltedOnError => break,
            }
        }

        self.finish_run_if_owner(room_id, run_id).await;
    }

    async fn finish_run_if_owner(&self, room_id: &str, run_id: u64) {
        let removed = {
            let mut runs = self.active_runs.lock().await;
            if runs.get(room_id).is_some_and(|r| r.run_id == run_id) {
                runs.remove(room_id);
                true
            } else {
                false
            }
        };
        if removed && let Ok(record) = self.store.get_record(room_id).await {
            self.sync_snapshot(&record);
            self.emit_turn_state(&record, None);
            self.emit_record_updated(&record);
        }
    }

    async fn resolve_next_step(
        &self,
        room_id: &str,
        last_speaker_seat: Option<&str>,
    ) -> Result<StepAction, RoomServiceError> {
        let selector_available = self.selector.is_available();
        let (_, action) = self
            .store
            .try_update_record(room_id, |rec| {
                if selector_available {
                    rec.window.decision_model_fallback = false;
                }
                if rec.status != RoomStatus::Active {
                    rec.window.queued_seats.clear();
                    rec.window.paused_at_cap = false;
                    return Ok::<_, RoomServiceError>(StepAction::StopIdle);
                }

                // 1. Pop queued @mention turns first (FR-2.2).
                while let Some(next_id) = pop_front_vec(&mut rec.window.queued_seats) {
                    let Some(seat) = rec.roster.iter().find(|s| s.seat_id == next_id).cloned()
                    else {
                        continue;
                    };
                    let used = rec.window.per_seat_used.get(&next_id).copied().unwrap_or(0);
                    if used >= MAX_SEAT_TURNS_PER_WINDOW {
                        continue;
                    }
                    if rec.window.window_used >= rec.window.window_cap {
                        rec.window.queued_seats.insert(0, next_id);
                        rec.window.paused_at_cap = true;
                        return Ok(StepAction::StopPausedAtCap);
                    }
                    return Ok(StepAction::RunSeat(seat));
                }

                // 2. Otherwise evaluate eligible seats for Decision Model (FR-2.1).
                let eligible: Vec<RoomSeat> = rec
                    .roster
                    .iter()
                    .filter(|seat| {
                        last_speaker_seat != Some(seat.seat_id.as_str())
                            && rec
                                .window
                                .per_seat_used
                                .get(&seat.seat_id)
                                .copied()
                                .unwrap_or(0)
                                < MAX_SEAT_TURNS_PER_WINDOW
                    })
                    .cloned()
                    .collect();
                if eligible.is_empty() {
                    rec.window.paused_at_cap = false;
                    return Ok(StepAction::StopIdle);
                }

                Ok(StepAction::ConsultSelector {
                    room_name: rec.name.clone(),
                    room_topic: rec.topic.clone(),
                    eligible,
                    at_cap: rec.window.window_used >= rec.window.window_cap,
                })
            })
            .await?;

        Ok(action)
    }

    async fn consult_selector_step(
        &self,
        room_id: &str,
        room_name: &str,
        room_topic: Option<&str>,
        eligible: &[RoomSeat],
        at_cap: bool,
        cancel_token: &CancellationToken,
    ) -> Option<RoomSeat> {
        let recent = self
            .store
            .read_messages(room_id, 12, None)
            .await
            .unwrap_or_default();

        let outcome = tokio::select! {
            biased;
            () = cancel_token.cancelled() => return None,
            outcome = self.selector.select_next(room_name, room_topic, &recent, eligible) => outcome,
        };

        if cancel_token.is_cancelled() {
            return None;
        }

        match outcome {
            SelectorOutcome::Unavailable => {
                if let Ok((rec, ())) = self
                    .store
                    .update_record(room_id, |rec| {
                        rec.window.decision_model_fallback = true;
                        rec.window.paused_at_cap = false;
                    })
                    .await
                {
                    self.sync_snapshot(&rec);
                    self.emit_record_updated(&rec);
                }
                None
            }
            SelectorOutcome::NoSpeaker => {
                if let Ok((rec, ())) = self
                    .store
                    .update_record(room_id, |rec| {
                        rec.window.decision_model_fallback = false;
                        rec.window.paused_at_cap = false;
                    })
                    .await
                {
                    self.sync_snapshot(&rec);
                    self.emit_record_updated(&rec);
                }
                None
            }
            SelectorOutcome::Pick(picked_id) => {
                let res = self
                    .store
                    .try_update_record(room_id, |rec| {
                        rec.window.decision_model_fallback = false;
                        let seat = rec.roster.iter().find(|s| s.seat_id == picked_id).cloned();
                        let used = rec
                            .window
                            .per_seat_used
                            .get(&picked_id)
                            .copied()
                            .unwrap_or(0);
                        match seat {
                            Some(seat) if used < MAX_SEAT_TURNS_PER_WINDOW => {
                                if at_cap || rec.window.window_used >= rec.window.window_cap {
                                    if !rec.window.queued_seats.contains(&seat.seat_id) {
                                        rec.window.queued_seats.push(seat.seat_id);
                                    }
                                    rec.window.paused_at_cap = true;
                                    Ok::<_, RoomServiceError>(None)
                                } else {
                                    Ok(Some(seat))
                                }
                            }
                            _ => {
                                rec.window.paused_at_cap = false;
                                Ok(None)
                            }
                        }
                    })
                    .await;
                match res {
                    Ok((rec, maybe_seat)) => {
                        self.sync_snapshot(&rec);
                        if maybe_seat.is_none() {
                            self.emit_record_updated(&rec);
                        }
                        maybe_seat
                    }
                    Err(_) => None,
                }
            }
        }
    }

    async fn execute_seat_turn(
        &self,
        room_id: &str,
        seat: &RoomSeat,
        cancel_token: &CancellationToken,
        active_turn_slot: &Arc<StdMutex<Option<RoomActiveTurn>>>,
    ) -> TurnStepOutcome {
        let Ok(record) = self.store.get_record(room_id).await else {
            return TurnStepOutcome::HaltedOnError;
        };
        let last_seen = record
            .last_seen_seq
            .get(&seat.seat_id)
            .copied()
            .unwrap_or(0);
        let all_msgs = self
            .store
            .read_all_messages(room_id)
            .await
            .unwrap_or_default();
        let missed: Vec<RoomMessage> = all_msgs.into_iter().filter(|m| m.seq > last_seen).collect();
        let latest_attachments = missed
            .last()
            .filter(|m| m.kind == RoomMessageKind::Owner)
            .map(|m| m.attachments.clone())
            .unwrap_or_default();
        let prompt = format_catchup_prompt(&record, &missed);

        let turn = RoomActiveTurn {
            seat_id: seat.seat_id.clone(),
            display_name: seat.display_name.clone(),
            seat_kind: seat.kind,
            started_at: Utc::now(),
            tool_status: None,
        };
        *active_turn_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(turn.clone());
        self.emit_turn_state(&record, Some(turn));

        let turn_result = match seat.kind {
            RoomSeatKind::Local => {
                self.run_local_seat_turn(
                    &record,
                    seat,
                    prompt,
                    latest_attachments,
                    cancel_token,
                    active_turn_slot,
                )
                .await
            }
            RoomSeatKind::SiblingA2a | RoomSeatKind::ExternalA2a => {
                let req = RemoteRoomTurnRequest {
                    seat_id: seat.seat_id.clone(),
                    room_id: room_id.to_owned(),
                    room_name: record.name.clone(),
                    prompt,
                };
                tokio::select! {
                    biased;
                    () = cancel_token.cancelled() => {
                        *active_turn_slot
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) = None;
                        return TurnStepOutcome::Cancelled;
                    }
                    res = self.remote.run_remote_turn(req) => res,
                }
            }
        };

        *active_turn_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        if cancel_token.is_cancelled() {
            return TurnStepOutcome::Cancelled;
        }

        match turn_result {
            Ok(output) => {
                self.handle_turn_success(room_id, seat, output).await;
                TurnStepOutcome::Completed {
                    speaker_seat_id: seat.seat_id.clone(),
                }
            }
            Err(err_msg) => {
                self.handle_turn_error(room_id, seat, &err_msg).await;
                TurnStepOutcome::HaltedOnError
            }
        }
    }

    async fn run_local_seat_turn(
        &self,
        record: &RoomRecord,
        seat: &RoomSeat,
        prompt: String,
        attachments: Vec<RoomAttachment>,
        cancel_token: &CancellationToken,
        active_turn_slot: &Arc<StdMutex<Option<RoomActiveTurn>>>,
    ) -> Result<RoomTurnOutput, String> {
        let Some(local_name) = seat.seat_id.strip_prefix("agent:") else {
            return Err("invalid local seat id".to_owned());
        };
        let Some(info) = self.get_local_agent(local_name) else {
            return Err(format!("agent '{}' is not running", seat.display_name));
        };

        let (tool_tx, mut tool_rx) = mpsc::unbounded_channel::<Option<String>>();
        let req = LocalRoomTurnRequest {
            room_id: record.id.to_string(),
            room_name: record.name.clone(),
            room_topic: record.topic.clone(),
            prompt,
            attachments,
            tool_status_tx: tool_tx,
        };
        let mut fut = std::pin::pin!(info.participant.run_room_turn(req));

        loop {
            tokio::select! {
                biased;
                () = cancel_token.cancelled() => {
                    return Err("turn canceled".to_owned());
                }
                res = &mut fut => {
                    return res;
                }
                maybe_status = tool_rx.recv() => {
                    if let Some(status) = maybe_status {
                        let updated_turn = {
                            let mut slot = active_turn_slot
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner);
                            slot.as_mut().map(|turn| {
                                turn.tool_status = status;
                                turn.clone()
                            })
                        };
                        if let Some(turn) = updated_turn {
                            self.emit_turn_state(record, Some(turn));
                        }
                    }
                }
            }
        }
    }

    async fn handle_turn_success(&self, room_id: &str, seat: &RoomSeat, output: RoomTurnOutput) {
        let Ok((_, message)) = self
            .store
            .append_message(
                room_id,
                NewRoomMessage {
                    kind: RoomMessageKind::Agent,
                    seat_id: Some(seat.seat_id.clone()),
                    author_name: seat.display_name.clone(),
                    seat_kind: Some(seat.kind),
                    content: output.content,
                    attachments: output.attachments,
                    timestamp: None,
                },
            )
            .await
        else {
            return;
        };

        if let Ok((updated, ())) = self
            .store
            .update_record(room_id, |rec| {
                rec.window.window_used = rec.window.window_used.saturating_add(1);
                let count = rec
                    .window
                    .per_seat_used
                    .entry(seat.seat_id.clone())
                    .or_insert(0);
                *count = count.saturating_add(1);

                let batons =
                    parse_agent_baton_mentions(&message.content, &seat.seat_id, &rec.roster);
                for target_id in batons {
                    let target_used = rec
                        .window
                        .per_seat_used
                        .get(&target_id)
                        .copied()
                        .unwrap_or(0);
                    if target_used < MAX_SEAT_TURNS_PER_WINDOW
                        && !rec.window.queued_seats.contains(&target_id)
                    {
                        rec.window.queued_seats.push(target_id);
                    }
                }
            })
            .await
        {
            self.sync_snapshot(&updated);
            self.emit(RoomEvent::Message {
                room_id: updated.id.clone(),
                message,
                unread_count: updated.unread_count(),
            });
            self.emit_turn_state(&updated, None);
            self.emit_record_updated(&updated);
        }
    }

    async fn handle_turn_error(&self, room_id: &str, seat: &RoomSeat, err_msg: &str) {
        let notice = format!("{} could not reply: {err_msg}", seat.display_name);
        let Ok((_, message)) = self
            .store
            .append_message(
                room_id,
                NewRoomMessage {
                    kind: RoomMessageKind::System,
                    seat_id: None,
                    author_name: "System".to_owned(),
                    seat_kind: None,
                    content: notice,
                    attachments: Vec::new(),
                    timestamp: None,
                },
            )
            .await
        else {
            return;
        };

        if let Ok((updated, ())) = self
            .store
            .update_record(room_id, |rec| {
                rec.window.queued_seats.clear();
                rec.window.paused_at_cap = false;
            })
            .await
        {
            self.sync_snapshot(&updated);
            self.emit(RoomEvent::Message {
                room_id: updated.id.clone(),
                message,
                unread_count: updated.unread_count(),
            });
            self.emit_turn_state(&updated, None);
            self.emit_record_updated(&updated);
        }
    }
}

enum TurnStepOutcome {
    Completed { speaker_seat_id: String },
    Cancelled,
    HaltedOnError,
}

fn pop_front_vec<T>(vec: &mut Vec<T>) -> Option<T> {
    (!vec.is_empty()).then(|| vec.remove(0))
}

fn normalize_optional_topic(topic: Option<String>) -> Option<String> {
    topic.and_then(|t| {
        let trimmed = t.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn slugify_room_name(name: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= MAX_MINUTES_SLUG_LEN {
            break;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn format_minutes_prompt(record: &RoomRecord, messages: &[RoomMessage]) -> String {
    let topic = record
        .topic
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("none");
    let roster = record
        .roster
        .iter()
        .map(|s| s.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut prompt = format!(
        "Generate concise, accurate markdown meeting minutes for the meeting room below.\n\
         Include a `# Meeting Minutes: {name}` title and sections:\n\
         `## Summary`\n\
         `## Decisions`\n\
         `## Action Items` (with owners where stated)\n\n\
         Base every statement strictly on the transcript below; do not invent decisions or owners.\n\n\
         Room: {name}\n\
         Topic: {topic}\n\
         Participants: {roster}\n\n\
         Transcript:\n",
        name = record.name
    );
    for msg in messages {
        _ = writeln!(prompt, "[{}]: {}", msg.author_name, msg.content);
    }
    prompt
}

fn fallback_minutes_markdown(record: &RoomRecord, messages: &[RoomMessage]) -> String {
    let topic = record
        .topic
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("none");
    let roster = record
        .roster
        .iter()
        .map(|s| s.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "# Meeting Minutes: {name}\n\n\
         **Topic:** {topic}\n\
         **Participants:** {roster}\n\n\
         ## Summary\n\
         Meeting archived after {count} message(s).\n\n\
         ## Decisions\n\
         - Review the archived transcript for recorded decisions.\n\n\
         ## Action Items\n\
         - Inspect room `{id}` for follow-ups.\n",
        name = record.name,
        count = messages.len(),
        id = record.id
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use residuum_agent::RoomRoster;
    use residuum_core::testing::gate::Gate;
    use residuum_core::testing::wait;
    use residuum_workspace::inbox;
    use residuum_workspace::testing::{StaticMailboxDirectory, TeamHub};

    use super::*;
    use crate::participants::{LocalRoomTurnRequest, RemoteRoomTurnRequest, RoomTurnOutput};
    use crate::selector::SelectorOutcome;
    use crate::types::{RoomMessageKind, RoomRosterCandidate, RoomSeat, RoomStatus};

    struct MockSelector {
        available: Mutex<bool>,
        outcomes: Mutex<VecDeque<SelectorOutcome>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl MockSelector {
        fn new(available: bool, outcomes: Vec<SelectorOutcome>) -> Self {
            Self {
                available: Mutex::new(available),
                outcomes: Mutex::new(VecDeque::from(outcomes)),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn recorded_calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SpeakerSelector for MockSelector {
        async fn select_next(
            &self,
            _room_name: &str,
            _room_topic: Option<&str>,
            _recent_messages: &[RoomMessage],
            eligible_seats: &[RoomSeat],
        ) -> SelectorOutcome {
            self.calls
                .lock()
                .unwrap()
                .push(eligible_seats.iter().map(|s| s.seat_id.clone()).collect());
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(SelectorOutcome::NoSpeaker)
        }

        fn is_available(&self) -> bool {
            *self.available.lock().unwrap()
        }
    }

    struct MockLocalParticipant {
        name: String,
        replies: Mutex<VecDeque<Result<String, String>>>,
        closed_rooms: Mutex<Vec<String>>,
        canceled_rooms: Mutex<Vec<String>>,
        minutes_text: Mutex<Option<String>>,
        hold_gate: Option<Gate>,
        minutes_gate: Option<Gate>,
        cancel_gate: Option<Gate>,
    }

    impl MockLocalParticipant {
        fn with_replies(name: &str, replies: Vec<Result<String, String>>) -> Arc<Self> {
            Arc::new(Self {
                name: name.to_owned(),
                replies: Mutex::new(VecDeque::from(replies)),
                closed_rooms: Mutex::new(Vec::new()),
                canceled_rooms: Mutex::new(Vec::new()),
                minutes_text: Mutex::new(None),
                hold_gate: None,
                minutes_gate: None,
                cancel_gate: None,
            })
        }

        fn holding(name: &str, gate: Gate) -> Arc<Self> {
            Arc::new(Self {
                name: name.to_owned(),
                replies: Mutex::new(VecDeque::new()),
                closed_rooms: Mutex::new(Vec::new()),
                canceled_rooms: Mutex::new(Vec::new()),
                minutes_text: Mutex::new(None),
                hold_gate: Some(gate),
                minutes_gate: None,
                cancel_gate: None,
            })
        }

        fn with_slow_minutes(
            name: &str,
            replies: Vec<Result<String, String>>,
            minutes_gate: Gate,
        ) -> Arc<Self> {
            Arc::new(Self {
                name: name.to_owned(),
                replies: Mutex::new(VecDeque::from(replies)),
                closed_rooms: Mutex::new(Vec::new()),
                canceled_rooms: Mutex::new(Vec::new()),
                minutes_text: Mutex::new(None),
                hold_gate: None,
                minutes_gate: Some(minutes_gate),
                cancel_gate: None,
            })
        }
    }

    #[async_trait]
    impl LocalRoomParticipant for MockLocalParticipant {
        async fn run_room_turn(&self, req: LocalRoomTurnRequest) -> Result<RoomTurnOutput, String> {
            req.tool_status_tx.send(Some("reading".to_owned())).ok();
            if let Some(gate) = &self.hold_gate {
                gate.entry().pass().await;
            }
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

        async fn cancel_room_turn(&self, room_id: &str) -> Result<(), String> {
            if let Some(gate) = &self.cancel_gate {
                gate.entry().pass().await;
            }
            self.canceled_rooms.lock().unwrap().push(room_id.to_owned());
            Ok(())
        }

        async fn close_room_session(&self, room_id: &str) -> Result<(), String> {
            self.closed_rooms.lock().unwrap().push(room_id.to_owned());
            Ok(())
        }

        async fn generate_minutes(&self, prompt: String) -> Result<String, String> {
            if let Some(gate) = &self.minutes_gate {
                gate.entry().pass().await;
            }
            let guard = self.minutes_text.lock().unwrap();
            if let Some(custom) = guard.as_ref() {
                return Ok(custom.clone());
            }
            Ok(format!(
                "# Meeting Minutes\n\n## Summary\nGenerated from {} chars.\n\n## Decisions\n- Shipped.\n\n## Action Items\n- Follow up.\n",
                prompt.len()
            ))
        }
    }

    struct MockRemoteDriver {
        candidates: Mutex<Vec<RoomRosterCandidate>>,
        closed_sessions: Mutex<Vec<(String, String)>>,
    }

    impl MockRemoteDriver {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                candidates: Mutex::new(Vec::new()),
                closed_sessions: Mutex::new(Vec::new()),
            })
        }

        fn with_candidates(candidates: Vec<RoomRosterCandidate>) -> Arc<Self> {
            Arc::new(Self {
                candidates: Mutex::new(candidates),
                closed_sessions: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl RemoteRoomDriver for MockRemoteDriver {
        async fn candidates(&self) -> Vec<RoomRosterCandidate> {
            self.candidates.lock().unwrap().clone()
        }

        async fn resolve_seat(&self, seat_id: &str) -> Option<RoomRosterCandidate> {
            self.candidates
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.seat_id == seat_id)
                .cloned()
        }

        async fn run_remote_turn(
            &self,
            _req: RemoteRoomTurnRequest,
        ) -> Result<RoomTurnOutput, String> {
            Ok(RoomTurnOutput {
                content: "Remote reply".to_owned(),
                attachments: Vec::new(),
            })
        }

        async fn cancel_remote_turn(&self, _room_id: &str, _seat_id: &str) -> Result<(), String> {
            Ok(())
        }

        async fn close_remote_session(&self, room_id: &str, seat_id: &str) -> Result<(), String> {
            self.closed_sessions
                .lock()
                .unwrap()
                .push((room_id.to_owned(), seat_id.to_owned()));
            Ok(())
        }
    }

    async fn wait_until_idle(
        rx: &mut broadcast::Receiver<RoomEvent>,
        room_id: &str,
        service: &RoomService,
    ) {
        wait::guarded("room turn loop to settle", async {
            loop {
                {
                    let runs = service.active_runs.lock().await;
                    if !runs.contains_key(room_id) {
                        return;
                    }
                }
                match rx.recv().await {
                    Ok(RoomEvent::TurnState {
                        room_id: id,
                        active_turn: None,
                        ..
                    }) if id.as_str() == room_id => {
                        let runs = service.active_runs.lock().await;
                        if !runs.contains_key(room_id) {
                            return;
                        }
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
        .await;
    }

    async fn setup_service(
        agents: &[(&str, &str, Arc<MockLocalParticipant>)],
        selector: Arc<MockSelector>,
    ) -> (TeamHub, Arc<RoomService>) {
        let hub = TeamHub::new();
        let names: Vec<&str> = agents.iter().map(|(n, _, _)| *n).collect();
        let mailboxes = Arc::new(StaticMailboxDirectory::for_team_hub(&hub, &names));
        let store = RoomStore::open(hub.dir.path().join("hub").join("rooms"))
            .await
            .unwrap();
        let service = RoomService::with_drivers(
            store,
            selector,
            MockRemoteDriver::empty(),
            hub.coordinator.clone(),
            mailboxes,
        )
        .await;
        for (name, display, participant) in agents {
            service.register_local_agent(LocalRoomAgentInfo {
                name: (*name).to_owned(),
                display_name: (*display).to_owned(),
                role: Some(format!("{display} role")),
                participant: Arc::clone(participant) as Arc<dyn LocalRoomParticipant>,
            });
        }
        (hub, service)
    }

    #[tokio::test]
    async fn unaddressed_message_picks_one_seat_and_stops_on_no_speaker() {
        let scout =
            MockLocalParticipant::with_replies("scout", vec![Ok("Checked logs.".to_owned())]);
        let analyst = MockLocalParticipant::with_replies("analyst", vec![]);
        let selector = Arc::new(MockSelector::new(
            true,
            vec![
                SelectorOutcome::Pick("agent:scout".to_owned()),
                SelectorOutcome::NoSpeaker,
            ],
        ));
        let (_hub, service) = setup_service(
            &[("scout", "Scout", scout), ("analyst", "Analyst", analyst)],
            Arc::clone(&selector),
        )
        .await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Morning Standup".to_owned(),
                topic: Some("Daily sync".to_owned()),
                seat_ids: vec!["agent:scout".to_owned(), "agent:analyst".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();

        service
            .post_owner_message(&room_id, "Any blockers today?".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let detail = service.get_room(&room_id, 50, None).await.unwrap();
        assert_eq!(detail.messages.len(), 2, "owner + one agent reply expected");
        assert_eq!(
            detail.messages.get(1).and_then(|m| m.seat_id.as_deref()),
            Some("agent:scout"),
            "scout should have spoken"
        );
        assert!(
            !detail.room.window.paused_at_cap,
            "stopping on NoSpeaker must not set paused_at_cap"
        );
    }

    #[tokio::test]
    async fn fallback_mode_skips_unaddressed_but_runs_explicit_and_baton_mentions() {
        let scout = MockLocalParticipant::with_replies(
            "scout",
            vec![Ok("Handing to @Analyst for numbers.".to_owned())],
        );
        let analyst = MockLocalParticipant::with_replies(
            "analyst",
            vec![Ok("Numbers look solid.".to_owned())],
        );
        let selector = Arc::new(MockSelector::new(false, vec![SelectorOutcome::Unavailable]));
        let (_hub, service) = setup_service(
            &[("scout", "Scout", scout), ("analyst", "Analyst", analyst)],
            selector,
        )
        .await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Review".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned(), "agent:analyst".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();

        service
            .post_owner_message(&room_id, "Unaddressed question".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let after_unaddressed = service.get_room(&room_id, 50, None).await.unwrap();
        assert_eq!(
            after_unaddressed.messages.len(),
            1,
            "unaddressed message in fallback mode must trigger 0 turns"
        );
        assert!(
            !after_unaddressed.decision_model_active,
            "decision_model_active must be false in fallback mode"
        );
        assert!(
            after_unaddressed.room.window.decision_model_fallback,
            "decision_model_fallback flag must be set"
        );

        service
            .post_owner_message(&room_id, "@Scout please check".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let after_mention = service.get_room(&room_id, 50, None).await.unwrap();
        assert_eq!(
            after_mention.messages.len(),
            4,
            "owner + scout + analyst baton-pass expected in fallback mode"
        );
        assert_eq!(
            after_mention
                .messages
                .get(2)
                .and_then(|m| m.seat_id.as_deref()),
            Some("agent:scout"),
            "scout should reply first"
        );
        assert_eq!(
            after_mention
                .messages
                .get(3)
                .and_then(|m| m.seat_id.as_deref()),
            Some("agent:analyst"),
            "analyst should follow via baton-pass in fallback mode"
        );
    }

    #[tokio::test]
    async fn consecutive_self_reply_guard_allows_solo_after_owner_and_explicit_mentions() {
        let scout = MockLocalParticipant::with_replies(
            "scout",
            vec![
                Ok("First pass.".to_owned()),
                Ok("Second pass after owner.".to_owned()),
                Ok("Third pass on mention.".to_owned()),
            ],
        );
        let selector = Arc::new(MockSelector::new(
            true,
            vec![
                SelectorOutcome::Pick("agent:scout".to_owned()),
                SelectorOutcome::Pick("agent:scout".to_owned()),
            ],
        ));
        let (_hub, service) =
            setup_service(&[("scout", "Scout", scout)], Arc::clone(&selector)).await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Solo Check".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();

        service
            .post_owner_message(&room_id, "Check 1".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;
        assert_eq!(
            service
                .get_room(&room_id, 50, None)
                .await
                .unwrap()
                .messages
                .len(),
            2,
            "solo agent should speak once after owner post and stop cleanly"
        );

        service
            .post_owner_message(&room_id, "Check 2".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;
        assert_eq!(
            service
                .get_room(&room_id, 50, None)
                .await
                .unwrap()
                .messages
                .len(),
            4,
            "solo agent can be auto-selected again right after a new owner message"
        );

        service
            .post_owner_message(&room_id, "@Scout one more".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;
        assert_eq!(
            service
                .get_room(&room_id, 50, None)
                .await
                .unwrap()
                .messages
                .len(),
            6,
            "explicit @mention must run even when same agent spoke prior agent turn"
        );
    }

    #[tokio::test]
    async fn window_cap_and_per_seat_cap_pause_and_continue_resets() {
        let a = MockLocalParticipant::with_replies(
            "a",
            vec![
                Ok("@B turn 1".to_owned()),
                Ok("@B turn 2".to_owned()),
                Ok("@B turn 3".to_owned()),
                Ok("Window 2 from A".to_owned()),
            ],
        );
        let b = MockLocalParticipant::with_replies(
            "b",
            vec![
                Ok("@A turn 1".to_owned()),
                Ok("@A turn 2".to_owned()),
                Ok("@A turn 3".to_owned()),
            ],
        );
        let c = MockLocalParticipant::with_replies("c", vec![Ok("C takes over".to_owned())]);
        let d = MockLocalParticipant::with_replies("d", vec![]);
        let selector = Arc::new(MockSelector::new(
            true,
            vec![
                SelectorOutcome::Pick("agent:c".to_owned()),
                SelectorOutcome::NoSpeaker,
            ],
        ));
        let (_hub, service) = setup_service(
            &[("a", "A", a), ("b", "B", b), ("c", "C", c), ("d", "D", d)],
            Arc::clone(&selector),
        )
        .await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Four Seat Cap".to_owned(),
                topic: None,
                seat_ids: vec![
                    "agent:a".to_owned(),
                    "agent:b".to_owned(),
                    "agent:c".to_owned(),
                    "agent:d".to_owned(),
                ],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();

        service
            .post_owner_message(&room_id, "@A start".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let paused = service.get_room(&room_id, 50, None).await.unwrap();
        assert_eq!(paused.room.window.window_cap, 6, "4 seats -> cap 6");
        assert_eq!(paused.room.window.window_used, 6, "6 turns should run");
        assert_eq!(
            paused.room.window.per_seat_used.get("agent:a").copied(),
            Some(3),
            "A should cap at 3 turns"
        );
        assert_eq!(
            paused.room.window.per_seat_used.get("agent:b").copied(),
            Some(3),
            "B should cap at 3 turns"
        );
        assert!(
            paused.room.window.paused_at_cap,
            "paused_at_cap must be true when C wanted to run at cap"
        );
        let calls = selector.recorded_calls();
        assert_eq!(
            calls.first().cloned().unwrap_or_default(),
            vec!["agent:c".to_owned(), "agent:d".to_owned()],
            "seats at 3 turns must be excluded from selector eligibility"
        );

        let continued = service.continue_window(&room_id).await.unwrap();
        assert!(
            !continued.window.paused_at_cap,
            "continue clears paused_at_cap"
        );
        assert_eq!(
            continued.window.window_used, 0,
            "continue resets window_used"
        );
        wait_until_idle(&mut rx, &room_id, &service).await;

        let after_continue = service.get_room(&room_id, 50, None).await.unwrap();
        assert_eq!(
            after_continue
                .messages
                .last()
                .and_then(|m| m.seat_id.as_deref()),
            Some("agent:c"),
            "C should run in the continued window"
        );
        assert!(
            !after_continue.room.window.paused_at_cap,
            "window stops cleanly on NoSpeaker"
        );
    }

    #[tokio::test]
    async fn stop_room_cancels_slow_turn_and_clears_queue_fast() {
        let gate = Gate::closed("slow-turn");
        let slow = MockLocalParticipant::holding("slow", gate);
        let next =
            MockLocalParticipant::with_replies("next", vec![Ok("Should not run".to_owned())]);
        let selector = Arc::new(MockSelector::new(true, Vec::new()));
        let (_hub, service) = setup_service(
            &[("slow", "Slow", Arc::clone(&slow)), ("next", "Next", next)],
            selector,
        )
        .await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Stop Test".to_owned(),
                topic: None,
                seat_ids: vec!["agent:slow".to_owned(), "agent:next".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();

        service
            .post_owner_message(&room_id, "@Slow @Next go".to_owned(), Vec::new())
            .await
            .unwrap();
        slow.hold_gate.as_ref().unwrap().until_held(1).await;

        let start = std::time::Instant::now();
        let stopped = service.stop_room(&room_id).await.unwrap();
        assert!(
            start.elapsed() < std::time::Duration::from_millis(200),
            "stop_room took {:?}, expected <200ms",
            start.elapsed()
        );
        assert!(stopped.active_turn.is_none(), "active_turn must be cleared");
        assert!(
            stopped.window.queued_seats.is_empty(),
            "queued_seats must be cleared on stop"
        );
        assert!(
            !slow.canceled_rooms.lock().unwrap().is_empty(),
            "cancel_room_turn must be invoked on the active local participant"
        );
    }

    #[tokio::test]
    async fn participant_failure_posts_system_notice_and_halts_chaining() {
        let scout = MockLocalParticipant::with_replies("scout", vec![Err("offline".to_owned())]);
        let analyst =
            MockLocalParticipant::with_replies("analyst", vec![Ok("Should never run".to_owned())]);
        let selector = Arc::new(MockSelector::new(true, Vec::new()));
        let (_hub, service) = setup_service(
            &[("scout", "Scout", scout), ("analyst", "Analyst", analyst)],
            selector,
        )
        .await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Failure Isolation".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned(), "agent:analyst".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();

        service
            .post_owner_message(&room_id, "@Scout @Analyst check".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let detail = service.get_room(&room_id, 50, None).await.unwrap();
        let [_, sys] = detail.messages.as_slice() else {
            panic!("owner + system notice only");
        };
        assert_eq!(sys.kind, RoomMessageKind::System, "must post System notice");
        assert!(
            sys.content.contains("Scout") && sys.content.contains("offline"),
            "system notice must name seat and error: {}",
            sys.content
        );
        assert!(
            detail.room.window.queued_seats.is_empty(),
            "remaining queue must be cleared after error halt"
        );
    }

    #[tokio::test]
    async fn empty_room_archive_writes_no_minutes_or_inbox() {
        let scout = MockLocalParticipant::with_replies("scout", Vec::new());
        let selector = Arc::new(MockSelector::new(true, Vec::new()));
        let (hub, service) = setup_service(&[("scout", "Scout", scout)], selector).await;

        let empty_room = service
            .create_room(CreateRoomRequest {
                name: "Empty Room".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let archived_empty = service
            .archive_room(empty_room.room.id.as_str())
            .await
            .unwrap();
        assert_eq!(
            archived_empty.room.status,
            RoomStatus::Archived,
            "status archived"
        );
        assert!(
            archived_empty.minutes_markdown.is_none(),
            "0-message room must not generate minutes"
        );
        assert!(
            archived_empty.room.minutes.shared_path.is_none(),
            "0-message room must not write team/minutes/"
        );
        assert!(
            inbox::list_items_or_empty(&hub.agent_dir("scout").join("inbox/user"))
                .await
                .unwrap()
                .is_empty(),
            "0-message room must not write an inbox item"
        );
    }

    async fn inbox_count(hub: &TeamHub, agent: &str) -> usize {
        inbox::list_items_or_empty(&hub.agent_dir(agent).join("inbox/user"))
            .await
            .unwrap()
            .len()
    }

    #[tokio::test]
    async fn archive_reopen_and_rearchive_minutes_behavior() {
        let scout = MockLocalParticipant::with_replies(
            "scout",
            vec![
                Ok("Discussed goals.".to_owned()),
                Ok("Follow-up after reopen.".to_owned()),
            ],
        );
        let selector = Arc::new(MockSelector::new(true, vec![SelectorOutcome::NoSpeaker]));
        let (hub, service) =
            setup_service(&[("scout", "Scout", Arc::clone(&scout))], selector).await;

        let room = service
            .create_room(CreateRoomRequest {
                name: "Architecture Sync".to_owned(),
                topic: Some("Modules".to_owned()),
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: true,
            })
            .await
            .unwrap();
        let room_id = room.room.id.to_string();
        let mut rx = service.subscribe_events();
        service
            .post_owner_message(&room_id, "@Scout summarize".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let archived = service.archive_room(&room_id).await.unwrap();
        let shared_path = archived.room.minutes.shared_path.clone().unwrap();
        assert!(
            shared_path.starts_with("team/minutes/")
                && shared_path.ends_with("-architecture-sync.md"),
            "unexpected shared_path: {shared_path}"
        );
        let rel = shared_path.strip_prefix("team/").unwrap();
        assert!(
            hub.team().join(rel).is_file(),
            "team/minutes file must exist on disk"
        );
        assert!(
            archived.minutes_markdown.is_some(),
            "minutes_markdown must be attached"
        );
        assert!(
            !scout.closed_rooms.lock().unwrap().is_empty(),
            "archive must close local room session"
        );
        assert_eq!(
            inbox_count(&hub, "scout").await,
            1,
            "one owner inbox item must be created"
        );

        service.reopen_room(&room_id).await.unwrap();
        let rearchived_no_new = service.archive_room(&room_id).await.unwrap();
        assert_eq!(
            rearchived_no_new.room.minutes.shared_path.as_deref(),
            Some(shared_path.as_str()),
            "shared_path preserved when 0 new messages"
        );
        assert_eq!(
            inbox_count(&hub, "scout").await,
            1,
            "0 new messages must not create duplicate inbox item"
        );

        service.reopen_room(&room_id).await.unwrap();
        *scout.minutes_text.lock().unwrap() =
            Some("# Meeting Minutes\n\n## Summary\nUpdated with follow-up.\n".to_owned());
        service
            .post_owner_message(&room_id, "@Scout follow up".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let rearchived_updated = service.archive_room(&room_id).await.unwrap();
        assert_eq!(
            rearchived_updated.room.minutes.shared_path.as_deref(),
            Some(shared_path.as_str()),
            "re-archive must update existing team/minutes path in place"
        );
        let disk_minutes = tokio::fs::read_to_string(hub.team().join(rel))
            .await
            .unwrap();
        assert!(
            disk_minutes.contains("Updated with follow-up"),
            "team/minutes file must be updated in place"
        );
        assert_eq!(
            inbox_count(&hub, "scout").await,
            2,
            "new message on re-archive must post updated inbox item"
        );
    }

    #[tokio::test]
    async fn room_agent_view_lists_reads_and_denies_outsiders() {
        let scout = MockLocalParticipant::with_replies("scout", vec![Ok("Notes.".to_owned())]);
        let selector = Arc::new(MockSelector::new(true, vec![SelectorOutcome::NoSpeaker]));
        let (_hub, service) = setup_service(&[("scout", "Scout", scout)], selector).await;

        let room = service
            .create_room(CreateRoomRequest {
                name: "Architecture Sync".to_owned(),
                topic: Some("Modules".to_owned()),
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: true,
            })
            .await
            .unwrap();
        let room_id = room.room.id.to_string();
        let mut rx = service.subscribe_events();
        service
            .post_owner_message(&room_id, "@Scout notes".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let archived = service.archive_room(&room_id).await.unwrap();
        let shared_path = archived.room.minutes.shared_path.unwrap();

        let view = service.agent_view("scout");
        let block = view.prompt_block().expect("scout participates in rooms");
        assert!(
            block.contains("Architecture Sync") && block.contains(&shared_path),
            "prompt block must include room and minutes path: {block}"
        );
        assert_eq!(view.list_for_agent().await.len(), 1, "scout is in 1 room");
        let read_res = view.read_for_agent(&room_id, 50).await.unwrap();
        assert_eq!(
            read_res.messages.len(),
            2,
            "read_for_agent returns transcript"
        );

        let outsider = service.agent_view("outsider");
        assert!(
            outsider.prompt_block().is_none(),
            "non-participant gets None prompt_block"
        );
        assert!(
            matches!(
                outsider.read_for_agent(&room_id, 50).await,
                Err(RoomServiceError::AccessDenied(_))
            ),
            "non-participant read_for_agent must be denied"
        );
    }

    #[tokio::test]
    async fn patch_topic_clearing_conflicts_mark_read_and_delete() {
        let scout = MockLocalParticipant::with_replies("scout", vec![Ok("ok".to_owned())]);
        let analyst = MockLocalParticipant::with_replies("analyst", vec![]);
        let selector = Arc::new(MockSelector::new(true, vec![SelectorOutcome::NoSpeaker]));
        let (_hub, service) = setup_service(
            &[("scout", "Scout", scout), ("analyst", "Analyst", analyst)],
            selector,
        )
        .await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Ops Check".to_owned(),
                topic: Some("Initial".to_owned()),
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();

        // None keeps topic; Some(" ") clears it; Some("New") sets trimmed.
        let kept = service
            .patch_room(
                &room_id,
                PatchRoomRequest {
                    pinned: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            kept.topic.as_deref(),
            Some("Initial"),
            "None topic must preserve existing topic"
        );
        let cleared = service
            .patch_room(
                &room_id,
                PatchRoomRequest {
                    topic: Some("   ".to_owned()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(cleared.topic, None, "whitespace topic must clear to None");

        let mut rx = service.subscribe_events();
        service
            .post_owner_message(&room_id, "@Scout ping".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        // continue_window when not paused must 409 Conflict.
        assert!(
            matches!(
                service.continue_window(&room_id).await,
                Err(RoomServiceError::Conflict(_))
            ),
            "continue when not paused must return Conflict"
        );

        let before_read = service.get_room(&room_id, 10, None).await.unwrap();
        assert_eq!(before_read.room.unread_count, 1, "agent reply is 1 unread");
        service.mark_read(&room_id, 2).await.unwrap();
        let after_read = service.get_room(&room_id, 10, None).await.unwrap();
        assert_eq!(after_read.room.unread_count, 0, "mark_read clears unread");

        service.archive_room(&room_id).await.unwrap();
        assert!(
            matches!(
                service
                    .post_owner_message(&room_id, "hi".to_owned(), Vec::new())
                    .await,
                Err(RoomServiceError::Conflict(_))
            ),
            "posting to archived room must return Conflict"
        );
        assert!(
            matches!(
                service
                    .patch_room(
                        &room_id,
                        PatchRoomRequest {
                            pinned: Some(false),
                            ..Default::default()
                        }
                    )
                    .await,
                Err(RoomServiceError::BadRequest(_))
            ),
            "patching archived room must return BadRequest"
        );

        service.delete_room(&room_id).await.unwrap();
        assert!(
            matches!(
                service.get_room(&room_id, 10, None).await,
                Err(RoomServiceError::NotFound(_))
            ),
            "deleted room must return NotFound"
        );
        assert!(
            service.agent_view("scout").prompt_block().is_none(),
            "deleted room must disappear from RoomAgentView"
        );
    }

    #[tokio::test]
    async fn post_owner_message_returns_conflict_while_archive_room_generates_minutes() {
        let minutes_gate = Gate::closed("slow-minutes");
        let scout = MockLocalParticipant::with_slow_minutes(
            "scout",
            vec![Ok("Summary input.".to_owned())],
            minutes_gate,
        );
        let selector = Arc::new(MockSelector::new(true, vec![SelectorOutcome::NoSpeaker]));
        let (_hub, service) =
            setup_service(&[("scout", "Scout", Arc::clone(&scout))], selector).await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Archive Race".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();
        service
            .post_owner_message(&room_id, "@Scout log this".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let svc_archive = Arc::clone(&service);
        let room_for_archive = room_id.clone();
        let archive_handle =
            tokio::spawn(async move { svc_archive.archive_room(&room_for_archive).await });

        scout.minutes_gate.as_ref().unwrap().until_held(1).await;

        let mid_archive_post = service
            .post_owner_message(&room_id, "late post".to_owned(), Vec::new())
            .await;
        assert!(
            matches!(mid_archive_post, Err(RoomServiceError::Conflict(_))),
            "post_owner_message during generate_minutes must return Conflict, got {mid_archive_post:?}"
        );
        assert_eq!(
            service
                .get_room(&room_id, 10, None)
                .await
                .unwrap()
                .room
                .status,
            RoomStatus::Archived,
            "room status must already be Archived before minutes generation finishes"
        );

        scout.minutes_gate.as_ref().unwrap().open_all();
        let archived = archive_handle.await.unwrap().unwrap();
        assert_eq!(
            archived.room.status,
            RoomStatus::Archived,
            "archive_room must finish Archived"
        );
        assert!(
            archived.minutes_markdown.is_some(),
            "archive_room must attach generated minutes"
        );
    }

    #[tokio::test]
    async fn stop_room_and_immediate_post_owner_message_preserves_new_mentions() {
        let gate = Gate::closed("slow-turn-for-stop-race");
        let cancel_gate = Gate::closed("slow-cancel-gate");
        let slow = Arc::new(MockLocalParticipant {
            name: "slow".to_owned(),
            replies: Mutex::new(VecDeque::new()),
            closed_rooms: Mutex::new(Vec::new()),
            canceled_rooms: Mutex::new(Vec::new()),
            minutes_text: Mutex::new(None),
            hold_gate: Some(gate),
            minutes_gate: None,
            cancel_gate: Some(cancel_gate),
        });
        let next = MockLocalParticipant::with_replies(
            "next",
            vec![Ok("Fresh mention handled.".to_owned())],
        );
        let selector = Arc::new(MockSelector::new(true, vec![SelectorOutcome::NoSpeaker]));
        let (_hub, service) = setup_service(
            &[("slow", "Slow", Arc::clone(&slow)), ("next", "Next", next)],
            selector,
        )
        .await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Stop Race".to_owned(),
                topic: None,
                seat_ids: vec!["agent:slow".to_owned(), "agent:next".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();

        service
            .post_owner_message(&room_id, "@Slow @Next first".to_owned(), Vec::new())
            .await
            .unwrap();
        slow.hold_gate.as_ref().unwrap().until_held(1).await;

        let svc_stop = Arc::clone(&service);
        let room_stop = room_id.clone();
        let stop_handle = tokio::spawn(async move { svc_stop.stop_room(&room_stop).await });
        slow.cancel_gate.as_ref().unwrap().until_held(1).await;

        let svc_post = Arc::clone(&service);
        let room_post = room_id.clone();
        let post_handle = tokio::spawn(async move {
            svc_post
                .post_owner_message(&room_post, "@Next second".to_owned(), Vec::new())
                .await
        });
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert!(
            !post_handle.is_finished(),
            "post_owner_message must serialize behind stop_room"
        );

        slow.cancel_gate.as_ref().unwrap().open_all();
        stop_handle.await.unwrap().unwrap();
        let post_res = post_handle.await.unwrap();
        assert!(
            post_res.is_ok(),
            "post_owner_message serialized behind stop_room must succeed: {post_res:?}"
        );
        wait_until_idle(&mut rx, &room_id, &service).await;

        let detail = service.get_room(&room_id, 50, None).await.unwrap();
        let last = detail.messages.last().unwrap();
        assert_eq!(
            last.seat_id.as_deref(),
            Some("agent:next"),
            "new @Next mention after stop must run and not be wiped by stop_room"
        );
        assert_eq!(
            last.content, "Fresh mention handled.",
            "new message's queued mention must produce its reply"
        );
    }

    #[tokio::test]
    async fn archive_room_closes_removed_historical_sibling_seat() {
        let hub = TeamHub::new();
        let mailboxes = Arc::new(StaticMailboxDirectory::for_team_hub(&hub, &["scout"]));
        let store = RoomStore::open(hub.dir.path().join("hub").join("rooms"))
            .await
            .unwrap();
        let remote = MockRemoteDriver::with_candidates(vec![
            RoomRosterCandidate {
                seat_id: "a2a:ops/planner".to_owned(),
                kind: RoomSeatKind::SiblingA2a,
                display_name: "Planner".to_owned(),
                role: Some("Ops".to_owned()),
                instance: Some("ops".to_owned()),
                online: true,
            },
            RoomRosterCandidate {
                seat_id: "a2a:partner".to_owned(),
                kind: RoomSeatKind::ExternalA2a,
                display_name: "Partner".to_owned(),
                role: Some("External".to_owned()),
                instance: None,
                online: true,
            },
        ]);
        let selector = Arc::new(MockSelector::new(true, vec![SelectorOutcome::NoSpeaker]));
        let service = RoomService::with_drivers(
            store,
            selector,
            Arc::clone(&remote) as Arc<dyn RemoteRoomDriver>,
            hub.coordinator.clone(),
            mailboxes,
        )
        .await;
        let scout = MockLocalParticipant::with_replies("scout", vec![Ok("Done.".to_owned())]);
        service.register_local_agent(LocalRoomAgentInfo {
            name: "scout".to_owned(),
            display_name: "Scout".to_owned(),
            role: Some("Scout role".to_owned()),
            participant: scout,
        });

        let created = service
            .create_room(CreateRoomRequest {
                name: "Sibling Close".to_owned(),
                topic: None,
                seat_ids: vec![
                    "agent:scout".to_owned(),
                    "a2a:ops/planner".to_owned(),
                    "a2a:partner".to_owned(),
                ],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();

        // Remove the sibling seat mid-meeting before archive.
        service
            .patch_room(
                &room_id,
                PatchRoomRequest {
                    remove_seats: Some(vec!["a2a:ops/planner".to_owned()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        service.archive_room(&room_id).await.unwrap();

        let closed = remote.closed_sessions.lock().unwrap().clone();
        assert_eq!(
            closed,
            vec![(room_id, "a2a:ops/planner".to_owned())],
            "removed historical sibling A2A seat must still be closed on archive, and external A2A must not"
        );
    }

    #[tokio::test]
    async fn enabling_decision_model_clears_fallback_on_existing_room() {
        let scout = MockLocalParticipant::with_replies("scout", Vec::new());
        let selector = Arc::new(MockSelector::new(false, vec![SelectorOutcome::Unavailable]));
        let (_hub, service) =
            setup_service(&[("scout", "Scout", scout)], Arc::clone(&selector)).await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Settings Toggle".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        assert!(
            !created.decision_model_active,
            "decision_model_active must be false when selector is unavailable"
        );
        assert!(
            !created.room.window.decision_model_fallback,
            "create_room must not pre-latch decision_model_fallback"
        );

        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();
        service
            .post_owner_message(&room_id, "Anyone there?".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let in_fallback = service.get_room(&room_id, 10, None).await.unwrap();
        assert!(
            in_fallback.room.window.decision_model_fallback,
            "Unavailable selector outcome must latch fallback"
        );
        assert!(
            !in_fallback.decision_model_active,
            "decision_model_active must remain false while unavailable"
        );

        *selector.available.lock().unwrap() = true;
        let recovered = service.get_room(&room_id, 10, None).await.unwrap();
        assert!(
            recovered.decision_model_active,
            "enabling System 1 must immediately make existing room decision_model_active"
        );
        assert!(
            !recovered.room.window.decision_model_fallback,
            "enabling System 1 must clear decision_model_fallback on existing room"
        );
    }

    #[tokio::test]
    async fn reopen_room_serializes_behind_in_flight_archive_room() {
        let minutes_gate = Gate::closed("slow-minutes-reopen");
        let scout = MockLocalParticipant::with_slow_minutes(
            "scout",
            vec![Ok("Notes.".to_owned())],
            minutes_gate,
        );
        let selector = Arc::new(MockSelector::new(true, vec![SelectorOutcome::NoSpeaker]));
        let (_hub, service) =
            setup_service(&[("scout", "Scout", Arc::clone(&scout))], selector).await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Reopen Serialize".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();
        service
            .post_owner_message(&room_id, "@Scout note".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let svc_archive = Arc::clone(&service);
        let room_archive = room_id.clone();
        let archive_handle =
            tokio::spawn(async move { svc_archive.archive_room(&room_archive).await });
        scout.minutes_gate.as_ref().unwrap().until_held(1).await;

        let svc_reopen = Arc::clone(&service);
        let room_reopen = room_id.clone();
        let reopen_handle = tokio::spawn(async move { svc_reopen.reopen_room(&room_reopen).await });
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert!(
            !reopen_handle.is_finished(),
            "reopen_room must wait for in-flight archive_room to finish"
        );

        scout.minutes_gate.as_ref().unwrap().open_all();
        let archived = archive_handle.await.unwrap().unwrap();
        assert!(
            archived.minutes_markdown.is_some(),
            "archive_room must finish generating minutes before reopen runs"
        );
        let reopened = reopen_handle.await.unwrap().unwrap();
        assert_eq!(
            reopened.status,
            RoomStatus::Active,
            "final status must be Active after serialized reopen_room"
        );
        assert_eq!(
            service
                .get_room(&room_id, 10, None)
                .await
                .unwrap()
                .room
                .status,
            RoomStatus::Active,
            "stored status must remain Active and not be overwritten by archive_room"
        );
    }

    #[tokio::test]
    async fn delete_room_serializes_behind_in_flight_archive_room() {
        let minutes_gate = Gate::closed("slow-minutes-delete");
        let scout = MockLocalParticipant::with_slow_minutes(
            "scout",
            vec![Ok("Notes.".to_owned())],
            minutes_gate,
        );
        let selector = Arc::new(MockSelector::new(true, vec![SelectorOutcome::NoSpeaker]));
        let (hub, service) =
            setup_service(&[("scout", "Scout", Arc::clone(&scout))], selector).await;

        let created = service
            .create_room(CreateRoomRequest {
                name: "Delete Serialize".to_owned(),
                topic: None,
                seat_ids: vec!["agent:scout".to_owned()],
                pinned: false,
            })
            .await
            .unwrap();
        let room_id = created.room.id.to_string();
        let mut rx = service.subscribe_events();
        service
            .post_owner_message(&room_id, "@Scout note".to_owned(), Vec::new())
            .await
            .unwrap();
        wait_until_idle(&mut rx, &room_id, &service).await;

        let svc_archive = Arc::clone(&service);
        let room_archive = room_id.clone();
        let archive_handle =
            tokio::spawn(async move { svc_archive.archive_room(&room_archive).await });
        scout.minutes_gate.as_ref().unwrap().until_held(1).await;

        let svc_delete = Arc::clone(&service);
        let room_delete = room_id.clone();
        let delete_handle = tokio::spawn(async move { svc_delete.delete_room(&room_delete).await });
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert!(
            !delete_handle.is_finished(),
            "delete_room must wait for in-flight archive_room to finish"
        );

        scout.minutes_gate.as_ref().unwrap().open_all();
        let archived = archive_handle.await.unwrap().unwrap();
        assert!(
            archived.minutes_markdown.is_some(),
            "archive_room must complete before delete_room executes"
        );
        delete_handle.await.unwrap().unwrap();

        assert!(
            matches!(
                service.get_room(&room_id, 10, None).await,
                Err(RoomServiceError::NotFound(_))
            ),
            "deleted room must return NotFound"
        );
        let room_dir = hub.dir.path().join("hub").join("rooms").join(&room_id);
        assert!(
            !room_dir.exists(),
            "delete_room after serialized archive_room must leave no orphan directory at {}",
            room_dir.display()
        );
        assert!(
            !service.room_locks.lock().await.contains_key(&room_id),
            "delete_room must clean up room_locks entry"
        );
    }
}
