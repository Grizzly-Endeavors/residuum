//! On-disk persistence for meeting rooms under `<hub_dir>/rooms/<room_id>/`.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use residuum_core::util::fs::atomic_write;
use thiserror::Error;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::RwLock;

use crate::types::{
    RoomAttachment, RoomMessage, RoomMessageKind, RoomRecord, RoomSeatKind, RoomSummary,
    RoomValidationError, generate_message_id, validate_room_id,
};

/// Errors returned by [`RoomStore`] operations.
#[derive(Debug, Error)]
pub enum RoomStoreError {
    /// The supplied room ID failed validation.
    #[error(transparent)]
    InvalidRoomId(#[from] RoomValidationError),
    /// No room exists with the requested ID.
    #[error("room '{0}' not found")]
    NotFound(String),
    /// A room with that ID already exists.
    #[error("room '{0}' already exists")]
    AlreadyExists(String),
    /// Filesystem I/O failure.
    #[error("room store io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or deserialization failure.
    #[error("room store serialization error: {0}")]
    Json(#[from] serde_json::Error),
    /// Atomic write failure.
    #[error("room store write error: {0}")]
    Write(String),
}

/// Input payload for appending a new message to a room's transcript.
#[derive(Debug, Clone)]
pub struct NewRoomMessage {
    /// Author kind (`owner`, `agent`, `system`).
    pub kind: RoomMessageKind,
    /// Authoring seat ID (`None` for owner/system).
    pub seat_id: Option<String>,
    /// Display name recorded on the message.
    pub author_name: String,
    /// Seat hosting kind (`None` for owner/system).
    pub seat_kind: Option<RoomSeatKind>,
    /// Markdown message text.
    pub content: String,
    /// Attached files/images.
    pub attachments: Vec<RoomAttachment>,
    /// Explicit timestamp override; defaults to `Utc::now()` when `None`.
    pub timestamp: Option<DateTime<Utc>>,
}

/// Persistent room store backed by `<rooms_root>/<room_id>/`.
#[derive(Debug)]
pub struct RoomStore {
    rooms_root: PathBuf,
    rooms: RwLock<BTreeMap<String, RoomRecord>>,
}

impl RoomStore {
    /// Open the room store at `rooms_root`, creating it if missing and loading all
    /// valid persisted rooms.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::Io`] if `rooms_root` cannot be created or read.
    pub async fn open(rooms_root: PathBuf) -> Result<Self, RoomStoreError> {
        tokio::fs::create_dir_all(&rooms_root).await?;
        let mut rooms = BTreeMap::new();
        let mut entries = tokio::fs::read_dir(&rooms_root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if !file_type.is_dir() {
                continue;
            }
            let dir_name = entry.file_name();
            let Some(dir_str) = dir_name.to_str() else {
                continue;
            };
            if dir_str.starts_with('.') {
                continue;
            }
            if validate_room_id(dir_str).is_err() {
                tracing::warn!(dir = %dir_str, "skipping non-room directory under rooms_root");
                continue;
            }
            let room_dir = entry.path();
            match load_room_from_dir(&room_dir, dir_str).await {
                Ok(record) => {
                    rooms.insert(record.id.to_string(), record);
                }
                Err(err) => {
                    tracing::warn!(dir = %room_dir.display(), error = %err, "failed to load room record on startup; skipping");
                }
            }
        }
        Ok(Self {
            rooms_root,
            rooms: RwLock::new(rooms),
        })
    }

    /// Root directory where room subdirectories are stored.
    #[must_use]
    pub fn rooms_root(&self) -> &Path {
        &self.rooms_root
    }

    /// List all room records sorted by `updated_at` descending.
    pub async fn list_records(&self) -> Vec<RoomRecord> {
        let guard = self.rooms.read().await;
        let mut list: Vec<RoomRecord> = guard.values().cloned().collect();
        drop(guard);
        list.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        list
    }

    /// List all room summaries sorted by `updated_at` descending.
    pub async fn list_summaries(&self) -> Vec<RoomSummary> {
        self.list_records()
            .await
            .iter()
            .map(RoomSummary::from_record)
            .collect()
    }

    /// Fetch a single room record by ID.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`] or [`RoomStoreError::NotFound`].
    pub async fn get_record(&self, room_id: &str) -> Result<RoomRecord, RoomStoreError> {
        validate_room_id(room_id)?;
        let guard = self.rooms.read().await;
        guard
            .get(room_id)
            .cloned()
            .ok_or_else(|| RoomStoreError::NotFound(room_id.to_owned()))
    }

    /// Persist a newly created room record atomically under `<rooms_root>/<id>/room.json`.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::AlreadyExists`] or an I/O/serialization error.
    pub async fn create_room(&self, record: RoomRecord) -> Result<RoomRecord, RoomStoreError> {
        let key = record.id.to_string();
        validate_room_id(&key)?;
        let mut guard = self.rooms.write().await;
        if guard.contains_key(&key) {
            return Err(RoomStoreError::AlreadyExists(key));
        }
        let dir = self.room_dir(&key);
        tokio::fs::create_dir_all(&dir).await?;
        write_room_json(&dir, &record).await?;
        guard.insert(key, record.clone());
        Ok(record)
    }

    /// Mutate an existing room record under the store write lock and persist it
    /// atomically when `mutate` returns `Ok`.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`], [`RoomStoreError::NotFound`], or write errors.
    pub async fn update_record<F, R>(
        &self,
        room_id: &str,
        mutate: F,
    ) -> Result<(RoomRecord, R), RoomStoreError>
    where
        F: FnOnce(&mut RoomRecord) -> R,
    {
        validate_room_id(room_id)?;
        let mut guard = self.rooms.write().await;
        let existing = guard
            .get(room_id)
            .ok_or_else(|| RoomStoreError::NotFound(room_id.to_owned()))?;
        let mut updated = existing.clone();
        let outcome = mutate(&mut updated);
        let dir = self.room_dir(room_id);
        write_room_json(&dir, &updated).await?;
        guard.insert(room_id.to_owned(), updated.clone());
        Ok((updated, outcome))
    }

    /// Advance `last_read_seq = max(last_read_seq, min(seq, last_seq))` and persist.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`], [`RoomStoreError::NotFound`], or write errors.
    pub async fn mark_read(&self, room_id: &str, seq: u64) -> Result<RoomRecord, RoomStoreError> {
        validate_room_id(room_id)?;
        let mut guard = self.rooms.write().await;
        let existing = guard
            .get(room_id)
            .ok_or_else(|| RoomStoreError::NotFound(room_id.to_owned()))?;
        let target = seq.min(existing.last_seq);
        if target <= existing.last_read_seq {
            return Ok(existing.clone());
        }
        let mut updated = existing.clone();
        updated.last_read_seq = target;
        let dir = self.room_dir(room_id);
        write_room_json(&dir, &updated).await?;
        guard.insert(room_id.to_owned(), updated.clone());
        Ok(updated)
    }

    /// Append a message to `transcript.jsonl`, increment `last_seq`, and persist `room.json`.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`], [`RoomStoreError::NotFound`], or I/O errors.
    pub async fn append_message(
        &self,
        room_id: &str,
        draft: NewRoomMessage,
    ) -> Result<(RoomRecord, RoomMessage), RoomStoreError> {
        validate_room_id(room_id)?;
        let mut guard = self.rooms.write().await;
        let existing = guard
            .get(room_id)
            .ok_or_else(|| RoomStoreError::NotFound(room_id.to_owned()))?;
        let mut updated = existing.clone();
        let seq = updated.last_seq.saturating_add(1);
        let timestamp = draft.timestamp.unwrap_or_else(Utc::now);
        let message = RoomMessage {
            seq,
            id: generate_message_id(),
            room_id: updated.id.clone(),
            kind: draft.kind,
            seat_id: draft.seat_id.clone(),
            author_name: draft.author_name,
            seat_kind: draft.seat_kind,
            content: draft.content,
            attachments: draft.attachments,
            timestamp,
        };

        let dir = self.room_dir(room_id);
        append_transcript_line(&dir, &message).await?;

        updated.last_seq = seq;
        if timestamp > updated.updated_at {
            updated.updated_at = timestamp;
        }
        if message.kind == RoomMessageKind::Owner {
            updated.last_read_seq = seq;
        }
        if message.kind == RoomMessageKind::Agent
            && let Some(seat_id) = &message.seat_id
        {
            updated.last_seen_seq.insert(seat_id.clone(), seq);
        }

        write_room_json(&dir, &updated).await?;
        guard.insert(room_id.to_owned(), updated.clone());
        Ok((updated, message))
    }

    /// Read up to `limit` messages (with `seq < before_seq` if supplied), ordered oldest-to-newest.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`], [`RoomStoreError::NotFound`], or I/O errors.
    pub async fn read_messages(
        &self,
        room_id: &str,
        limit: usize,
        before_seq: Option<u64>,
    ) -> Result<Vec<RoomMessage>, RoomStoreError> {
        validate_room_id(room_id)?;
        {
            let guard = self.rooms.read().await;
            if !guard.contains_key(room_id) {
                return Err(RoomStoreError::NotFound(room_id.to_owned()));
            }
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let path = self.room_dir(room_id).join("transcript.jsonl");
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(RoomStoreError::Io(err)),
        };

        let mut filtered = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: RoomMessage = serde_json::from_str(trimmed)?;
            if before_seq.is_some_and(|bound| msg.seq >= bound) {
                continue;
            }
            filtered.push(msg);
        }
        if filtered.len() > limit {
            let start = filtered.len() - limit;
            filtered.drain(..start);
        }
        Ok(filtered)
    }

    /// Read all persisted messages for a room, ordered oldest-to-newest.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`], [`RoomStoreError::NotFound`], or I/O errors.
    pub async fn read_all_messages(
        &self,
        room_id: &str,
    ) -> Result<Vec<RoomMessage>, RoomStoreError> {
        self.read_messages(room_id, usize::MAX, None).await
    }

    /// Atomically write `<rooms_root>/<room_id>/minutes.md`.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`], [`RoomStoreError::NotFound`], or write errors.
    pub async fn write_minutes(&self, room_id: &str, markdown: &str) -> Result<(), RoomStoreError> {
        validate_room_id(room_id)?;
        {
            let guard = self.rooms.read().await;
            if !guard.contains_key(room_id) {
                return Err(RoomStoreError::NotFound(room_id.to_owned()));
            }
        }
        let path = self.room_dir(room_id).join("minutes.md");
        atomic_write(&path, markdown.as_bytes())
            .await
            .map_err(|err| RoomStoreError::Write(err.to_string()))
    }

    /// Read `<rooms_root>/<room_id>/minutes.md`, returning `Ok(None)` if not written yet.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`], [`RoomStoreError::NotFound`], or I/O errors.
    pub async fn read_minutes(&self, room_id: &str) -> Result<Option<String>, RoomStoreError> {
        validate_room_id(room_id)?;
        {
            let guard = self.rooms.read().await;
            if !guard.contains_key(room_id) {
                return Err(RoomStoreError::NotFound(room_id.to_owned()));
            }
        }
        let path = self.room_dir(room_id).join("minutes.md");
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(RoomStoreError::Io(err)),
        }
    }

    /// Delete `<rooms_root>/<room_id>/` and remove the room from memory.
    ///
    /// # Errors
    /// Returns [`RoomStoreError::InvalidRoomId`], [`RoomStoreError::NotFound`], or I/O errors.
    pub async fn delete_room(&self, room_id: &str) -> Result<(), RoomStoreError> {
        validate_room_id(room_id)?;
        let mut guard = self.rooms.write().await;
        if guard.remove(room_id).is_none() {
            return Err(RoomStoreError::NotFound(room_id.to_owned()));
        }
        let dir = self.room_dir(room_id);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(RoomStoreError::Io(err)),
        }
    }

    fn room_dir(&self, room_id: &str) -> PathBuf {
        self.rooms_root.join(room_id)
    }
}

async fn write_room_json(room_dir: &Path, record: &RoomRecord) -> Result<(), RoomStoreError> {
    let bytes = serde_json::to_vec_pretty(record)?;
    atomic_write(&room_dir.join("room.json"), &bytes)
        .await
        .map_err(|err| RoomStoreError::Write(err.to_string()))
}

async fn append_transcript_line(
    room_dir: &Path,
    message: &RoomMessage,
) -> Result<(), RoomStoreError> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(room_dir.join("transcript.jsonl"))
        .await?;
    file.write_all(line.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

async fn load_room_from_dir(room_dir: &Path, dir_name: &str) -> Result<RoomRecord, RoomStoreError> {
    let raw = tokio::fs::read_to_string(room_dir.join("room.json")).await?;
    let mut record: RoomRecord = serde_json::from_str(&raw)?;
    if record.id.as_str() != dir_name {
        return Err(RoomStoreError::InvalidRoomId(
            RoomValidationError::InvalidRoomId(record.id.to_string()),
        ));
    }
    let transcript_path = room_dir.join("transcript.jsonl");
    match tokio::fs::read_to_string(&transcript_path).await {
        Ok(text) => {
            if let Some(last_line) = text.lines().rev().find(|l| !l.trim().is_empty())
                && let Ok(last_msg) = serde_json::from_str::<RoomMessage>(last_line.trim())
                && last_msg.seq > record.last_seq
            {
                tracing::warn!(
                    room_id = %record.id,
                    room_json_seq = record.last_seq,
                    transcript_seq = last_msg.seq,
                    "transcript.jsonl ahead of room.json on startup; advancing last_seq"
                );
                record.last_seq = last_msg.seq;
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(RoomStoreError::Io(err)),
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RoomMinutesMeta, RoomSeat, RoomStatus, RoomWindowState, generate_room_id};

    fn sample_record(now: DateTime<Utc>) -> RoomRecord {
        let id = generate_room_id();
        RoomRecord {
            id,
            name: "Design Sync".to_owned(),
            topic: Some("Wire types and store".to_owned()),
            status: RoomStatus::Active,
            pinned: true,
            created_at: now,
            updated_at: now,
            archived_at: None,
            roster: vec![
                RoomSeat {
                    seat_id: "agent:scout".to_owned(),
                    kind: RoomSeatKind::Local,
                    display_name: "Scout".to_owned(),
                    role: Some("Research".to_owned()),
                    instance: None,
                },
                RoomSeat {
                    seat_id: "a2a:ops/planner".to_owned(),
                    kind: RoomSeatKind::SiblingA2a,
                    display_name: "Planner".to_owned(),
                    role: Some("Ops".to_owned()),
                    instance: Some("ops".to_owned()),
                },
            ],
            historical_participants: vec!["agent:scout".to_owned(), "a2a:ops/planner".to_owned()],
            last_seq: 0,
            last_read_seq: 0,
            last_seen_seq: BTreeMap::new(),
            window: RoomWindowState::fresh(2),
            minutes: RoomMinutesMeta::default(),
        }
    }

    fn owner_msg(content: &str, timestamp: DateTime<Utc>) -> NewRoomMessage {
        NewRoomMessage {
            kind: RoomMessageKind::Owner,
            seat_id: None,
            author_name: "Owner".to_owned(),
            seat_kind: None,
            content: content.to_owned(),
            attachments: Vec::new(),
            timestamp: Some(timestamp),
        }
    }

    fn agent_msg(
        seat_id: &str,
        author_name: &str,
        seat_kind: RoomSeatKind,
        content: &str,
        timestamp: DateTime<Utc>,
    ) -> NewRoomMessage {
        NewRoomMessage {
            kind: RoomMessageKind::Agent,
            seat_id: Some(seat_id.to_owned()),
            author_name: author_name.to_owned(),
            seat_kind: Some(seat_kind),
            content: content.to_owned(),
            attachments: Vec::new(),
            timestamp: Some(timestamp),
        }
    }

    #[tokio::test]
    async fn room_store_create_append_and_reload() {
        let temp = tempfile::tempdir().unwrap();
        let rooms_root = temp.path().join("rooms");
        let now = Utc::now();

        let store = RoomStore::open(rooms_root.clone()).await.unwrap();
        assert!(
            store.list_records().await.is_empty(),
            "fresh store should start empty"
        );

        let record = sample_record(now);
        let room_id = record.id.to_string();
        store.create_room(record).await.unwrap();
        let loaded = store.get_record(&room_id).await.unwrap();
        assert_eq!(loaded.name, "Design Sync", "created room name must match");

        let (rec1, msg1) = store
            .append_message(&room_id, owner_msg("@everyone status?", now))
            .await
            .unwrap();
        assert_eq!(msg1.seq, 1, "first message seq must be 1");
        assert_eq!(
            rec1.last_read_seq, 1,
            "owner post must advance last_read_seq"
        );
        assert_eq!(rec1.unread_count(), 0, "owner post must have 0 unread");

        let (rec2, msg2) = store
            .append_message(
                &room_id,
                agent_msg(
                    "agent:scout",
                    "Scout",
                    RoomSeatKind::Local,
                    "All clear.",
                    now,
                ),
            )
            .await
            .unwrap();
        assert_eq!(msg2.seq, 2, "second message seq must be 2");
        assert_eq!(
            rec2.unread_count(),
            1,
            "agent reply should increment unread"
        );
        assert_eq!(
            rec2.last_seen_seq.get("agent:scout").copied(),
            Some(2),
            "agent reply should update seat last_seen_seq"
        );

        let (rec3, msg3) = store
            .append_message(
                &room_id,
                agent_msg(
                    "a2a:ops/planner",
                    "Planner",
                    RoomSeatKind::SiblingA2a,
                    "Ready on ops.",
                    now,
                ),
            )
            .await
            .unwrap();
        assert_eq!(msg3.seq, 3, "third message seq must be 3");
        assert_eq!(
            rec3.unread_count(),
            2,
            "two agent replies should yield unread=2"
        );

        let reloaded = RoomStore::open(rooms_root).await.unwrap();
        let reloaded_rec = reloaded.get_record(&room_id).await.unwrap();
        assert_eq!(
            reloaded_rec.last_seq, 3,
            "reloaded room must preserve last_seq=3"
        );
        assert_eq!(
            reloaded_rec.unread_count(),
            2,
            "reloaded room must preserve unread_count=2"
        );

        let slice = reloaded.read_messages(&room_id, 2, None).await.unwrap();
        assert_eq!(
            slice.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![2, 3],
            "slice must be oldest-to-newest tail"
        );

        let before = reloaded.read_messages(&room_id, 10, Some(3)).await.unwrap();
        assert_eq!(
            before.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![1, 2],
            "before_seq=3 must return seqs 1 and 2"
        );
    }

    #[tokio::test]
    async fn room_store_mark_read_minutes_and_delete() {
        let temp = tempfile::tempdir().unwrap();
        let rooms_root = temp.path().join("rooms");
        let now = Utc::now();

        let store = RoomStore::open(rooms_root.clone()).await.unwrap();
        let record = sample_record(now);
        let room_id = record.id.to_string();
        store.create_room(record).await.unwrap();

        for idx in 1..=3 {
            store
                .append_message(
                    &room_id,
                    NewRoomMessage {
                        kind: RoomMessageKind::Agent,
                        seat_id: Some("agent:scout".to_owned()),
                        author_name: "Scout".to_owned(),
                        seat_kind: Some(RoomSeatKind::Local),
                        content: format!("msg {idx}"),
                        attachments: Vec::new(),
                        timestamp: Some(now),
                    },
                )
                .await
                .unwrap();
        }

        let partial = store.mark_read(&room_id, 2).await.unwrap();
        assert_eq!(
            partial.unread_count(),
            1,
            "marking seq 2 read leaves 1 unread"
        );
        let full = store.mark_read(&room_id, 99).await.unwrap();
        assert_eq!(full.last_read_seq, 3, "mark_read must clamp to last_seq");
        assert_eq!(full.unread_count(), 0, "full mark_read clears unread");

        assert_eq!(
            store.read_minutes(&room_id).await.unwrap(),
            None,
            "minutes should be None before write"
        );
        store
            .write_minutes(&room_id, "# Minutes\n\n- Agreed.\n")
            .await
            .unwrap();
        assert_eq!(
            store.read_minutes(&room_id).await.unwrap().as_deref(),
            Some("# Minutes\n\n- Agreed.\n"),
            "written minutes must be read back verbatim"
        );

        store.delete_room(&room_id).await.unwrap();
        assert!(
            matches!(
                store.get_record(&room_id).await,
                Err(RoomStoreError::NotFound(_))
            ),
            "deleted room must return NotFound"
        );
        assert!(
            !rooms_root.join(&room_id).exists(),
            "deleted room directory must be removed from disk"
        );
    }

    #[tokio::test]
    async fn room_store_rejects_traversal_room_ids() {
        let temp = tempfile::tempdir().unwrap();
        let store = RoomStore::open(temp.path().join("rooms")).await.unwrap();
        assert!(
            matches!(
                store.get_record("../etc").await,
                Err(RoomStoreError::InvalidRoomId(_))
            ),
            "traversal room id must fail validation"
        );
    }
}
