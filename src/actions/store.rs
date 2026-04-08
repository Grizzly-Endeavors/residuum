//! Persistent storage for scheduled actions (`scheduled_actions.json`).
//!
//! Uses atomic write (temp file + rename) to prevent corruption on crash.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tracing::debug;

use anyhow::Context;

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
    /// Returns an error if the file exists but cannot be read or is not valid JSON.
    #[tracing::instrument(skip_all)]
    pub async fn load(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                let actions: Vec<ScheduledAction> =
                    serde_json::from_str(&contents).with_context(|| {
                        format!("failed to parse scheduled actions at {}", path.display())
                    })?;
                debug!(path = %path.display(), count = actions.len(), "loaded scheduled actions");
                Ok(Self { actions, path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                actions: Vec::new(),
                path,
            }),
            Err(e) => Err(e)
                .with_context(|| format!("failed to read scheduled actions at {}", path.display())),
        }
    }

    /// Save the store to disk atomically (write temp file, then rename).
    ///
    /// # Errors
    /// Returns an error if serialization or writing fails.
    #[tracing::instrument(skip_all, fields(path = %self.path.display(), count = self.actions.len()))]
    pub async fn save(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(&self.actions)
            .context("failed to serialize scheduled actions")?;

        let dir = self.path.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "scheduled actions path has no parent directory: {}",
                self.path.display()
            )
        })?;

        tokio::fs::create_dir_all(dir).await.with_context(|| {
            format!(
                "failed to create directory for scheduled actions at {}",
                dir.display()
            )
        })?;

        crate::util::fs::atomic_write(&self.path, &json).await?;

        debug!(path = %self.path.display(), count = self.actions.len(), "saved scheduled actions");
        Ok(())
    }

    /// Create an in-memory store backed by the given path (not yet saved).
    ///
    /// Used as a fallback when the actions file cannot be loaded, and in tests.
    #[must_use]
    pub fn new_empty(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            actions: Vec::new(),
            path,
        }
    }

    /// Add an action to the store (does not save; call [`save`] separately).
    pub fn add(&mut self, action: ScheduledAction) {
        debug!(id = %action.id, name = %action.name, run_at = %action.run_at, "scheduled action added");
        self.actions.push(action);
    }

    /// Remove an action by ID. Returns true if the action was found and removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.actions.len();
        self.actions.retain(|a| a.id != id);
        let found = self.actions.len() < before;
        if found {
            debug!(id = %id, "scheduled action removed");
        } else {
            debug!(id = %id, "attempted to remove action that does not exist in store");
        }
        found
    }

    /// List all pending actions.
    #[must_use]
    pub fn list(&self) -> &[ScheduledAction] {
        &self.actions
    }

    /// Drain and return all actions whose `run_at` is at or before `now`.
    #[must_use]
    pub fn take_due(&mut self, now: DateTime<Utc>) -> Vec<ScheduledAction> {
        let (due, remaining) = self.actions.drain(..).partition(|a| a.run_at <= now);
        self.actions = remaining;
        if !due.is_empty() {
            debug!(count = due.len(), "draining due actions");
        }
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

        let original = make_action("action-00000001", 60);
        let mut store = ActionStore::load(&path).await.unwrap();
        store.add(original.clone());
        store.save().await.unwrap();

        let loaded = ActionStore::load(&path).await.unwrap();
        assert_eq!(loaded.list().len(), 1, "should load one action");
        assert_eq!(
            loaded.list().first().unwrap(),
            &original,
            "loaded action should equal original"
        );
    }

    #[tokio::test]
    async fn round_trip_optional_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");

        let now = Utc::now();
        let original = ScheduledAction {
            id: "action-00000002".to_string(),
            name: "action with options".to_string(),
            prompt: "do something".to_string(),
            run_at: now + Duration::seconds(60),
            agent: Some("main".to_string()),
            model_tier: Some("small".to_string()),
            created_at: now,
        };

        let mut store = ActionStore::new_empty(&path);
        store.add(original.clone());
        store.save().await.unwrap();

        let loaded = ActionStore::load(&path).await.unwrap();
        assert_eq!(loaded.list().len(), 1);
        assert_eq!(
            loaded.list().first().unwrap(),
            &original,
            "optional fields should survive round-trip"
        );
    }

    #[test]
    fn take_due_filters_correctly() {
        let now = Utc::now();
        let mut store = ActionStore::new_empty(PathBuf::from("/tmp/test.json"));
        store.add(make_action("past", -60));
        store.add(make_action("future", 3600));
        store.add(make_action("also-past", -1));

        let due = store.take_due(now);
        assert_eq!(due.len(), 2, "should take 2 due actions");
        assert_eq!(store.list().len(), 1, "1 future action should remain");
        assert_eq!(store.list().first().map(|a| a.id.as_str()), Some("future"));
    }

    #[test]
    fn take_due_empty_store() {
        let mut store = ActionStore::new_empty(PathBuf::from("/tmp/test.json"));
        let due = store.take_due(Utc::now());
        assert!(due.is_empty(), "empty store should return no due actions");
        assert!(store.list().is_empty(), "store should remain empty");
    }

    #[test]
    fn take_due_exact_now_boundary() {
        let mut store = ActionStore::new_empty(PathBuf::from("/tmp/test.json"));
        store.add(make_action("exact", 0));
        let now = Utc::now();
        let due = store.take_due(now);
        assert_eq!(due.len(), 1, "action at exactly now should be taken");
        assert!(
            store.list().is_empty(),
            "store should be empty after taking"
        );
    }

    #[test]
    fn remove_by_id() {
        let mut store = ActionStore::new_empty(PathBuf::from("/tmp/test.json"));
        store.add(make_action("keep", 60));
        store.add(make_action("remove", 120));

        assert!(store.remove("remove"), "should return true for existing");
        assert!(!store.remove("remove"), "should return false for missing");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list().first().map(|a| a.id.as_str()), Some("keep"));
    }

    #[test]
    fn generate_id_format() {
        let id = ScheduledAction::generate_id();
        assert!(id.starts_with("action-"), "id should start with 'action-'");
        assert_eq!(id.len(), 15, "id should be action- + 8 hex chars");
    }

    #[tokio::test]
    async fn atomic_write_no_tmp_remains() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");
        let store = ActionStore::load(&path).await.unwrap();
        store.save().await.unwrap();

        assert!(path.exists(), "saved file should exist");
        let tmp_files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            tmp_files.is_empty(),
            "no .tmp files should remain after save"
        );
    }

    #[tokio::test]
    async fn malformed_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");
        tokio::fs::write(&path, "not json").await.unwrap();
        let result = ActionStore::load(&path).await;
        assert!(result.is_err(), "malformed JSON should return error");
        let err = result.err().unwrap();
        assert!(
            err.to_string().contains(path.to_str().unwrap()),
            "error should contain the file path"
        );
    }
}
