//! Persistent storage for cron jobs (`cron/jobs.json`).
//!
//! Uses atomic write (temp file + rename) to prevent corruption on crash.

use std::path::{Path, PathBuf};

use rand::Rng;

use crate::error::IronclawError;

use super::types::CronJob;

/// Storage for cron jobs backed by a JSON file.
pub struct CronStore {
    jobs: Vec<CronJob>,
    path: PathBuf,
}

impl CronStore {
    /// Load the store from disk.
    ///
    /// Returns an empty store if the file does not exist.
    ///
    /// # Errors
    /// Returns `IronclawError::Scheduling` if the file exists but cannot be
    /// read or is not valid JSON.
    pub async fn load(path: impl Into<PathBuf>) -> Result<Self, IronclawError> {
        let path = path.into();
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                let jobs: Vec<CronJob> = serde_json::from_str(&contents).map_err(|e| {
                    IronclawError::Scheduling(format!(
                        "failed to parse cron jobs at {}: {e}",
                        path.display()
                    ))
                })?;
                Ok(Self { jobs, path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                jobs: Vec::new(),
                path,
            }),
            Err(e) => Err(IronclawError::Scheduling(format!(
                "failed to read cron jobs at {}: {e}",
                path.display()
            ))),
        }
    }

    /// Save the store to disk atomically (write temp file, then rename).
    ///
    /// # Errors
    /// Returns `IronclawError::Scheduling` if serialization or writing fails.
    pub async fn save(&self) -> Result<(), IronclawError> {
        let json = serde_json::to_string_pretty(&self.jobs).map_err(|e| {
            IronclawError::Scheduling(format!("failed to serialize cron jobs: {e}"))
        })?;

        let dir = self.path.parent().ok_or_else(|| {
            IronclawError::Scheduling(format!(
                "cron jobs path has no parent directory: {}",
                self.path.display()
            ))
        })?;

        let tmp_path = dir.join(".jobs.json.tmp");

        tokio::fs::write(&tmp_path, &json).await.map_err(|e| {
            IronclawError::Scheduling(format!(
                "failed to write temporary cron jobs at {}: {e}",
                tmp_path.display()
            ))
        })?;

        tokio::fs::rename(&tmp_path, &self.path)
            .await
            .map_err(|e| {
                IronclawError::Scheduling(format!(
                    "failed to rename cron jobs from {} to {}: {e}",
                    tmp_path.display(),
                    self.path.display()
                ))
            })?;

        Ok(())
    }

    /// Create an in-memory store backed by the given path (not yet saved).
    ///
    /// Useful for tests where a live file is not needed.
    #[cfg(test)]
    pub fn new_empty(path: impl Into<PathBuf>) -> Self {
        Self {
            jobs: Vec::new(),
            path: path.into(),
        }
    }

    /// Generate a unique job ID in the form `cron-{8 hex digits}`.
    #[must_use]
    pub fn generate_id() -> String {
        format!("cron-{:08x}", rand::thread_rng().r#gen::<u32>())
    }

    /// Add a job to the store (does not save; call [`save`] separately).
    pub fn add_job(&mut self, job: CronJob) {
        self.jobs.push(job);
    }

    /// Update an existing job by ID. Returns true if the job was found.
    pub fn update_job(&mut self, job: CronJob) -> bool {
        if let Some(existing) = self.jobs.iter_mut().find(|j| j.id == job.id) {
            *existing = job;
            true
        } else {
            false
        }
    }

    /// Remove a job by ID. Returns true if the job was found and removed.
    pub fn remove_job(&mut self, id: &str) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|j| j.id != id);
        self.jobs.len() < before
    }

    /// Get a reference to a job by ID.
    #[must_use]
    pub fn get_job(&self, id: &str) -> Option<&CronJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Get a mutable reference to a job by ID.
    #[must_use]
    pub fn get_job_mut(&mut self, id: &str) -> Option<&mut CronJob> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// List all jobs.
    #[must_use]
    pub fn list_jobs(&self) -> &[CronJob] {
        &self.jobs
    }

    /// Find jobs that are enabled and whose `next_run_at` is at or before `now`.
    #[must_use]
    pub fn find_due_jobs(&self, now: chrono::DateTime<chrono::Utc>) -> Vec<&CronJob> {
        self.jobs
            .iter()
            .filter(|j| j.enabled && j.state.next_run_at.is_some_and(|next| next <= now))
            .collect()
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
    use chrono::{TimeZone, Utc};

    use crate::cron::types::{CronJobState, CronPayload, CronSchedule, RunStatus, SessionTarget};

    fn make_job(id: &str) -> CronJob {
        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        CronJob {
            id: id.to_string(),
            name: format!("job {id}"),
            description: None,
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule: CronSchedule::At { at: now },
            session_target: SessionTarget::Main,
            payload: CronPayload::SystemEvent {
                text: "event".to_string(),
            },
            state: CronJobState::default(),
        }
    }

    #[tokio::test]
    async fn load_missing_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let store = CronStore::load(path).await.unwrap();
        assert!(
            store.list_jobs().is_empty(),
            "missing file should give empty store"
        );
    }

    #[tokio::test]
    async fn round_trip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");

        let mut store = CronStore::load(&path).await.unwrap();
        store.add_job(make_job("cron-00000001"));
        store.save().await.unwrap();

        let loaded = CronStore::load(&path).await.unwrap();
        assert_eq!(loaded.list_jobs().len(), 1, "should load one job");
        assert_eq!(
            loaded.list_jobs().first().map(|j| j.id.as_str()),
            Some("cron-00000001"),
            "job id should match"
        );
    }

    #[tokio::test]
    async fn add_update_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let mut store = CronStore::load(&path).await.unwrap();

        store.add_job(make_job("cron-aaa"));
        assert_eq!(store.list_jobs().len(), 1, "should have one job after add");

        let mut updated = make_job("cron-aaa");
        updated.name = "updated name".to_string();
        let found = store.update_job(updated);
        assert!(found, "update should return true for existing job");
        assert_eq!(
            store.get_job("cron-aaa").map(|j| j.name.as_str()),
            Some("updated name"),
            "name should be updated"
        );

        let removed = store.remove_job("cron-aaa");
        assert!(removed, "remove should return true for existing job");
        assert!(
            store.list_jobs().is_empty(),
            "store should be empty after remove"
        );
    }

    #[tokio::test]
    async fn find_due_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let mut store = CronStore::load(&path).await.unwrap();

        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        let past = Utc.with_ymd_and_hms(2026, 2, 19, 11, 0, 0).unwrap();
        let future = Utc.with_ymd_and_hms(2026, 2, 19, 13, 0, 0).unwrap();

        let mut due_job = make_job("cron-due");
        due_job.state.next_run_at = Some(past);

        let mut future_job = make_job("cron-future");
        future_job.state.next_run_at = Some(future);

        let mut no_next = make_job("cron-no-next");
        no_next.state.next_run_at = None;

        store.add_job(due_job);
        store.add_job(future_job);
        store.add_job(no_next);

        let due = store.find_due_jobs(now);
        assert_eq!(due.len(), 1, "only the past-due job should be found");
        assert_eq!(
            due.first().map(|j| j.id.as_str()),
            Some("cron-due"),
            "due job id should match"
        );
    }

    #[tokio::test]
    async fn disabled_jobs_not_due() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let mut store = CronStore::load(&path).await.unwrap();

        let now = Utc.with_ymd_and_hms(2026, 2, 19, 12, 0, 0).unwrap();
        let past = Utc.with_ymd_and_hms(2026, 2, 19, 11, 0, 0).unwrap();

        let mut job = make_job("cron-disabled");
        job.enabled = false;
        job.state.next_run_at = Some(past);
        store.add_job(job);

        let due = store.find_due_jobs(now);
        assert!(due.is_empty(), "disabled jobs should not be found");
    }

    #[tokio::test]
    async fn atomic_write_no_tmp_remains() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let store = CronStore::load(&path).await.unwrap();
        store.save().await.unwrap();

        let tmp = dir.path().join(".jobs.json.tmp");
        assert!(!tmp.exists(), "tmp file should not remain after save");
    }

    #[test]
    fn generate_id_format() {
        let id = CronStore::generate_id();
        assert!(id.starts_with("cron-"), "id should start with 'cron-'");
        assert_eq!(id.len(), 13, "id should be cron- + 8 hex chars");
    }

    #[tokio::test]
    async fn malformed_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        tokio::fs::write(&path, "not json").await.unwrap();
        let result = CronStore::load(&path).await;
        assert!(result.is_err(), "malformed JSON should return error");
    }

    #[test]
    fn update_nonexistent_returns_false() {
        let mut store = CronStore::new_empty("/tmp/fake.json");
        let job = make_job("cron-ghost");
        assert!(
            !store.update_job(job),
            "updating nonexistent job should return false"
        );
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let mut store = CronStore::new_empty("/tmp/fake.json");
        assert!(
            !store.remove_job("cron-ghost"),
            "removing nonexistent job should return false"
        );
    }

    #[test]
    fn run_status_serializes() {
        let json = serde_json::to_string(&RunStatus::Ok).unwrap();
        assert_eq!(json, "\"ok\"", "ok should serialize as 'ok'");
    }
}
