//! Persistent storage for scheduled actions (`scheduled_actions.json`).
//!
//! Uses atomic write (temp file + rename) to prevent corruption on crash.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rand::Rng;

use crate::error::ResiduumError;

use super::types::ScheduledAction;

/// Storage for scheduled actions backed by a JSON file.
pub struct ActionStore {
    actions: Vec<ScheduledAction>,
    path: PathBuf,
}

impl ActionStore {
    /// Load the store from disk.
    ///
    /// Returns an empty store if the file does not exist.
    ///
    /// # Errors
    /// Returns `ResiduumError::Scheduling` if the file exists but cannot be
    /// read or is not valid JSON.
    pub async fn load(path: impl Into<PathBuf>) -> Result<Self, ResiduumError> {
        let path = path.into();
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                let actions: Vec<ScheduledAction> =
                    serde_json::from_str(&contents).map_err(|e| {
                        ResiduumError::Scheduling(format!(
                            "failed to parse scheduled actions at {}: {e}",
                            path.display()
                        ))
                    })?;
                Ok(Self { actions, path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                actions: Vec::new(),
                path,
            }),
            Err(e) => Err(ResiduumError::Scheduling(format!(
                "failed to read scheduled actions at {}: {e}",
                path.display()
            ))),
        }
    }

    /// Save the store to disk atomically (write temp file, then rename).
    ///
    /// # Errors
    /// Returns `ResiduumError::Scheduling` if serialization or writing fails.
    pub async fn save(&self) -> Result<(), ResiduumError> {
        let json = serde_json::to_string_pretty(&self.actions).map_err(|e| {
            ResiduumError::Scheduling(format!("failed to serialize scheduled actions: {e}"))
        })?;

        let dir = self.path.parent().ok_or_else(|| {
            ResiduumError::Scheduling(format!(
                "scheduled actions path has no parent directory: {}",
                self.path.display()
            ))
        })?;

        // Ensure the parent directory exists (actions file lives at workspace root,
        // which should already exist, but be defensive)
        if !dir.exists() {
            tokio::fs::create_dir_all(dir).await.map_err(|e| {
                ResiduumError::Scheduling(format!(
                    "failed to create directory for scheduled actions at {}: {e}",
                    dir.display()
                ))
            })?;
        }

        let tmp_path = dir.join(".scheduled_actions.json.tmp");

        tokio::fs::write(&tmp_path, &json).await.map_err(|e| {
            ResiduumError::Scheduling(format!(
                "failed to write temporary scheduled actions at {}: {e}",
                tmp_path.display()
            ))
        })?;

        tokio::fs::rename(&tmp_path, &self.path)
            .await
            .map_err(|e| {
                ResiduumError::Scheduling(format!(
                    "failed to rename scheduled actions from {} to {}: {e}",
                    tmp_path.display(),
                    self.path.display()
                ))
            })?;

        Ok(())
    }

    /// Create an in-memory store backed by the given path (not yet saved).
    ///
    /// Used as a fallback when the actions file cannot be loaded, and in tests.
    #[must_use]
    pub fn new_empty(path: impl Into<PathBuf>) -> Self {
        Self {
            actions: Vec::new(),
            path: path.into(),
        }
    }

    /// Generate a unique action ID in the form `action-{8 hex digits}`.
    #[must_use]
    pub fn generate_id() -> String {
        format!("action-{:08x}", rand::thread_rng().r#gen::<u32>())
    }

    /// Add an action to the store (does not save; call [`save`] separately).
    pub fn add(&mut self, action: ScheduledAction) {
        self.actions.push(action);
    }

    /// Remove an action by ID. Returns true if the action was found and removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.actions.len();
        self.actions.retain(|a| a.id != id);
        self.actions.len() < before
    }

    /// List all pending actions.
    #[must_use]
    pub fn list(&self) -> &[ScheduledAction] {
        &self.actions
    }

    /// Drain and return all actions whose `run_at` is at or before `now`.
    #[must_use]
    pub fn take_due(&mut self, now: DateTime<Utc>) -> Vec<ScheduledAction> {
        let mut due = Vec::new();
        let mut remaining = Vec::new();

        for action in self.actions.drain(..) {
            if action.run_at <= now {
                due.push(action);
            } else {
                remaining.push(action);
            }
        }

        self.actions = remaining;
        due
    }

    /// Path to the backing JSON file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_action(id: &str, offset_secs: i64) -> ScheduledAction {
        let now = Utc::now();
        ScheduledAction {
            id: id.to_string(),
            name: format!("action {id}"),
            prompt: "do something".to_string(),
            run_at: now + Duration::seconds(offset_secs),
            agent: None,
            model_tier: None,
            channels: vec!["agent_feed".to_string()],
            created_at: now,
        }
    }

    #[tokio::test]
    async fn load_missing_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");
        let store = ActionStore::load(path).await.unwrap();
        assert!(
            store.list().is_empty(),
            "missing file should give empty store"
        );
    }

    #[tokio::test]
    async fn round_trip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");

        let mut store = ActionStore::load(&path).await.unwrap();
        store.add(make_action("action-00000001", 60));
        store.save().await.unwrap();

        let loaded = ActionStore::load(&path).await.unwrap();
        assert_eq!(loaded.list().len(), 1, "should load one action");
        assert_eq!(
            loaded.list().first().map(|a| a.id.as_str()),
            Some("action-00000001"),
            "action id should match"
        );
    }

    #[test]
    fn take_due_filters_correctly() {
        let now = Utc::now();
        let mut store = ActionStore {
            actions: vec![
                make_action("past", -60),
                make_action("future", 3600),
                make_action("also-past", -1),
            ],
            path: PathBuf::from("/tmp/test.json"),
        };

        let due = store.take_due(now);
        assert_eq!(due.len(), 2, "should take 2 due actions");
        assert_eq!(store.list().len(), 1, "1 future action should remain");
        assert_eq!(store.list().first().map(|a| a.id.as_str()), Some("future"));
    }

    #[test]
    fn remove_by_id() {
        let mut store = ActionStore {
            actions: vec![make_action("keep", 60), make_action("remove", 120)],
            path: PathBuf::from("/tmp/test.json"),
        };

        assert!(store.remove("remove"), "should return true for existing");
        assert!(!store.remove("remove"), "should return false for missing");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list().first().map(|a| a.id.as_str()), Some("keep"));
    }

    #[test]
    fn generate_id_format() {
        let id = ActionStore::generate_id();
        assert!(id.starts_with("action-"), "id should start with 'action-'");
        assert_eq!(id.len(), 15, "id should be action- + 8 hex chars");
    }

    #[tokio::test]
    async fn atomic_write_no_tmp_remains() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");
        let store = ActionStore::load(&path).await.unwrap();
        store.save().await.unwrap();

        let tmp = dir.path().join(".scheduled_actions.json.tmp");
        assert!(!tmp.exists(), "tmp file should not remain after save");
    }

    #[tokio::test]
    async fn malformed_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");
        tokio::fs::write(&path, "not json").await.unwrap();
        let result = ActionStore::load(&path).await;
        assert!(result.is_err(), "malformed JSON should return error");
    }
}
