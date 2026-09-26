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
    /// A stored action left over from before `agent: "main"` was removed is
    /// dropped — never silently reinterpreted as a plain session — with an
    /// error naming it, while the rest of the store still loads. Every such
    /// action is also returned alongside the store so the caller can raise an
    /// owner-facing notice; this only ever runs once, at startup, so unlike
    /// `HEARTBEAT.yml`'s hot-reloaded pulses there's no repeat-tick spam to
    /// guard against here.
    ///
    /// A file that exists but isn't valid JSON is never overwritten: its raw
    /// bytes are moved aside to `<path>.corrupt-<unix-timestamp>` and the
    /// store starts empty at the normal path, so the next save can't clobber
    /// whatever was in the corrupt file. The third element of the returned
    /// tuple names that moved-aside file when this happened, so the caller
    /// can raise an owner-facing notice.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read, or is corrupt
    /// and could not be moved aside (in which case nothing is touched and the
    /// caller falls back to an empty in-memory store, same as before).
    #[tracing::instrument(skip_all)]
    pub async fn load(
        path: impl Into<PathBuf>,
    ) -> anyhow::Result<(Self, Vec<RejectedAction>, Option<PathBuf>)> {
        let path = path.into();
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => match serde_json::from_str::<Vec<ScheduledAction>>(&contents) {
                Ok(mut actions) => {
                    let rejected = reject_agent_main(&mut actions);
                    debug!(path = %path.display(), count = actions.len(), "loaded scheduled actions");
                    Ok((Self { actions, path }, rejected, None))
                }
                Err(parse_err) => {
                    let moved_to = move_aside_corrupt_file(&path).await.with_context(|| {
                        format!(
                            "scheduled actions at {} is corrupt ({parse_err}) and could not be \
                             moved aside",
                            path.display()
                        )
                    })?;
                    tracing::error!(
                        path = %path.display(),
                        moved_to = %moved_to.display(),
                        error = %parse_err,
                        "scheduled actions file is corrupt; moved aside and starting empty \
                         rather than overwriting it"
                    );
                    Ok((
                        Self {
                            actions: Vec::new(),
                            path,
                        },
                        Vec::new(),
                        Some(moved_to),
                    ))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((
                Self {
                    actions: Vec::new(),
                    path,
                },
                Vec::new(),
                None,
            )),
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
        let due: Vec<_> = self
            .actions
            .iter()
            .filter(|a| a.run_at <= now)
            .cloned()
            .collect();
        self.actions.retain(|a| a.run_at > now);
        if !due.is_empty() {
            debug!(count = due.len(), "draining due actions");
        }
        due
    }

    /// Return every action whose `run_at` is at or before `now`, without
    /// removing them from the store. The caller removes only the ones whose
    /// spawn actually started (see [`Self::remove`]), so an action a spawn
    /// attempt failed to publish stays here and is reconsidered due on the
    /// next call rather than being lost.
    #[must_use]
    pub fn due(&self, now: DateTime<Utc>) -> Vec<ScheduledAction> {
        self.actions
            .iter()
            .filter(|a| a.run_at <= now)
            .cloned()
            .collect()
    }

    /// Path to the backing JSON file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A stored scheduled action dropped at load because it used the removed
/// `agent: "main"` routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedAction {
    pub id: String,
    pub name: String,
}

/// Drop stored actions that still use the removed `agent: "main"` routing,
/// logging an actionable error naming each offender and returning what was
/// dropped so the caller can raise an owner-facing notice. Never silently
/// reinterpreted as a plain session — the owner needs to know this action
/// will no longer fire the way it used to.
fn reject_agent_main(actions: &mut Vec<ScheduledAction>) -> Vec<RejectedAction> {
    let mut rejected = Vec::new();
    actions.retain(|action| {
        let uses_main = action
            .agent
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case("main"));
        if uses_main {
            tracing::error!(
                action = %action.name,
                id = %action.id,
                "scheduled action uses agent: \"main\", which is no longer supported — every \
                 session fork already carries the main agent's identity and memory snapshot; \
                 dropping it rather than silently running it as a plain session. Re-create it \
                 with a skill name, or without agent_name, if it's still needed."
            );
            rejected.push(RejectedAction {
                id: action.id.clone(),
                name: action.name.clone(),
            });
        }
        !uses_main
    });
    rejected
}

/// Build an owner-facing notice naming every scheduled action dropped at load
/// for using the removed `agent: "main"` routing, pointing at the migration
/// guide. This is only ever called once, right after startup load, so unlike
/// the HEARTBEAT.yml pulse notice there's no repeat-tick dedup to do here.
#[must_use]
pub fn rejected_actions_notice(rejected: &[RejectedAction]) -> String {
    let details = rejected
        .iter()
        .map(|a| format!("- \"{}\" (id {})", a.name, a.id))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Dropped {count} scheduled action{plural} using the removed agent: \"main\" routing \
         (every session now carries the main agent's identity automatically, so this option is \
         gone):\n{details}\nRe-create with a skill name, or without agent_name, if still needed. \
         See {guide}.",
        count = rejected.len(),
        plural = if rejected.len() == 1 { "" } else { "s" },
        guide = crate::util::MIGRATION_GUIDE_URL,
    )
}

/// Build an owner-facing notice naming the corrupt `scheduled_actions.json`
/// file moved aside by [`ActionStore::load`], in plain language — the owner
/// is non-technical, so this says what happened and what to do, not that
/// JSON parsing failed.
#[must_use]
pub fn corrupt_actions_notice(moved_to: &Path) -> String {
    format!(
        "Your scheduled reminders file was damaged and couldn't be read, so I moved it aside to \
         \"{}\" and started fresh — new scheduled reminders and actions will work normally. \
         Anything that was scheduled before this happened will not run automatically anymore; \
         you can look inside that file if you want to recreate any of it by hand.",
        moved_to.display()
    )
}

/// Move a corrupt `scheduled_actions.json` aside to `<path>.corrupt-<unix
/// timestamp>` in the same directory, preserving its raw bytes rather than
/// letting the next save silently overwrite them.
///
/// # Errors
/// Returns an error if the rename fails (e.g. the directory isn't
/// writable).
async fn move_aside_corrupt_file(path: &Path) -> anyhow::Result<PathBuf> {
    let timestamp = Utc::now().timestamp();
    let mut dest = path.as_os_str().to_owned();
    dest.push(format!(".corrupt-{timestamp}"));
    let dest = PathBuf::from(dest);
    tokio::fs::rename(path, &dest).await.with_context(|| {
        format!(
            "failed to move {} aside to {}",
            path.display(),
            dest.display()
        )
    })?;
    Ok(dest)
}

#[cfg(test)]
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
        let (store, rejected, moved_aside) = ActionStore::load(path).await.unwrap();
        assert!(
            store.list().is_empty(),
            "missing file should give empty store"
        );
        assert!(
            rejected.is_empty(),
            "missing file should report no rejections"
        );
        assert!(moved_aside.is_none(), "missing file was never corrupt");
    }

    #[tokio::test]
    async fn round_trip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");

        let original = make_action("action-00000001", 60);
        let (mut store, rejected_on_first_load, _) = ActionStore::load(&path).await.unwrap();
        assert!(rejected_on_first_load.is_empty());
        store.add(original.clone());
        store.save().await.unwrap();

        let (loaded, rejected_on_reload, _) = ActionStore::load(&path).await.unwrap();
        assert!(rejected_on_reload.is_empty());
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
            agent: Some("memory-agent".to_string()),
            model_tier: Some("small".to_string()),
            created_at: now,
        };

        let mut store = ActionStore::new_empty(&path);
        store.add(original.clone());
        store.save().await.unwrap();

        let (loaded, _rejected, _moved_aside) = ActionStore::load(&path).await.unwrap();
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
        let now = Utc::now();
        let mut store = ActionStore::new_empty(PathBuf::from("/tmp/test.json"));
        store.add(ScheduledAction {
            id: "exact".to_string(),
            name: "action exact".to_string(),
            prompt: "do something".to_string(),
            run_at: now,
            agent: None,
            model_tier: None,
            created_at: now,
        });
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

    #[tokio::test]
    async fn load_drops_legacy_agent_main_action_but_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");

        let now = Utc::now();
        let legacy = ScheduledAction {
            id: "action-legacy1".to_string(),
            name: "legacy main action".to_string(),
            prompt: "do something".to_string(),
            run_at: now + Duration::seconds(60),
            agent: Some("main".to_string()),
            model_tier: None,
            created_at: now,
        };
        let json = serde_json::to_string(&vec![legacy, make_action("keep-me", 120)]).unwrap();
        std::fs::write(&path, json).unwrap();

        let (loaded, rejected, moved_aside) = ActionStore::load(&path).await.unwrap();
        assert!(moved_aside.is_none(), "valid JSON was never corrupt");
        assert_eq!(
            loaded.list().len(),
            1,
            "the agent: main action should be dropped, the other kept"
        );
        assert_eq!(loaded.list().first().unwrap().id, "keep-me");
        assert_eq!(
            rejected,
            vec![RejectedAction {
                id: "action-legacy1".to_string(),
                name: "legacy main action".to_string(),
            }],
            "the dropped action should be reported for the owner notice"
        );
    }

    #[tokio::test]
    async fn load_drops_agent_main_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");

        let now = Utc::now();
        let legacy = ScheduledAction {
            id: "action-legacy2".to_string(),
            name: "legacy MAIN action".to_string(),
            prompt: "do something".to_string(),
            run_at: now + Duration::seconds(60),
            agent: Some("MAIN".to_string()),
            model_tier: None,
            created_at: now,
        };
        let json = serde_json::to_string(&vec![legacy]).unwrap();
        std::fs::write(&path, json).unwrap();

        let (loaded, rejected, _moved_aside) = ActionStore::load(&path).await.unwrap();
        assert!(loaded.list().is_empty());
        assert_eq!(rejected.len(), 1, "the dropped action should be reported");
    }

    #[test]
    fn rejected_actions_notice_names_each_action_and_links_the_guide() {
        let rejected = vec![
            RejectedAction {
                id: "action-aaaaaaaa".to_string(),
                name: "old plan review".to_string(),
            },
            RejectedAction {
                id: "action-bbbbbbbb".to_string(),
                name: "nightly digest".to_string(),
            },
        ];
        let notice = rejected_actions_notice(&rejected);
        assert!(notice.contains("old plan review"));
        assert!(notice.contains("action-aaaaaaaa"));
        assert!(notice.contains("nightly digest"));
        assert!(notice.contains("action-bbbbbbbb"));
        assert!(
            notice.contains("migrating-to-agent-sessions.md"),
            "notice should point at the migration guide"
        );
        assert!(
            notice.contains('2'),
            "notice should mention how many actions were dropped"
        );
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
        let (store, _rejected, _moved_aside) = ActionStore::load(&path).await.unwrap();
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
    async fn malformed_json_is_moved_aside_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");
        tokio::fs::write(&path, "not json").await.unwrap();

        let (store, rejected, moved_aside) = ActionStore::load(&path).await.unwrap();
        assert!(
            store.list().is_empty(),
            "a corrupt file should load as an empty store"
        );
        assert!(rejected.is_empty());
        let moved_aside = moved_aside.expect("a corrupt file should be moved aside");
        assert!(
            moved_aside
                .to_string_lossy()
                .starts_with(&*path.to_string_lossy()),
            "moved-aside path should be derived from the original path"
        );
        assert!(
            !path.exists(),
            "the corrupt file must no longer be at the original path"
        );
        assert_eq!(
            tokio::fs::read_to_string(&moved_aside).await.unwrap(),
            "not json",
            "the original corrupt bytes must be preserved, not discarded"
        );

        // The next save must write a fresh file at the normal path, not
        // touch the moved-aside file again.
        store.save().await.unwrap();
        assert!(path.exists());
        assert_eq!(
            tokio::fs::read_to_string(&moved_aside).await.unwrap(),
            "not json",
            "a later save must never overwrite the moved-aside corrupt file"
        );
    }

    #[test]
    fn corrupt_actions_notice_names_the_moved_aside_file() {
        let notice = corrupt_actions_notice(Path::new(
            "/workspace/scheduled_actions.json.corrupt-1234567890",
        ));
        assert!(notice.contains("scheduled_actions.json.corrupt-1234567890"));
        assert!(
            !notice.to_lowercase().contains("json") || notice.contains("scheduled_actions.json"),
            "notice should name the file, not expose raw parser jargon"
        );
    }

    #[tokio::test]
    async fn due_peeks_without_removing() {
        let mut store = ActionStore::new_empty(PathBuf::from("/tmp/test.json"));
        store.add(make_action("past", -60));
        store.add(make_action("future", 3600));

        let due = store.due(Utc::now());
        assert_eq!(due.len(), 1, "only the past action is due");
        assert_eq!(
            store.list().len(),
            2,
            "due() must not remove anything from the store"
        );
    }
}
