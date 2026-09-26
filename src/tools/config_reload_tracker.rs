//! Correlates the agent's own config-file writes with the reload they
//! trigger, so the reload's outcome can be delivered back to the agent's own
//! transcript instead of only published as a user-facing notice.
//!
//! Wired only into main's `write_file`/`edit_file` tools (see
//! `gateway::startup::tools::init_tool_registry`) — a session's tool
//! registry never gets a [`ConfigWriteWatch`], since there is no live,
//! durable `Agent` to deliver a later note back into once a session's run
//! completes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::workspace::layout::WorkspaceLayout;

/// Which reload pathway a recognized config path funnels into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigReloadKind {
    /// `config.toml` / `providers.toml` — `ReloadSignal::Root`.
    Root,
    /// `mcp.json` / `channels.toml` / `a2a.json` — `ReloadSignal::Workspace`.
    Workspace,
    /// `HEARTBEAT.yml` — hot-reloaded on the pulse scheduler's next tick,
    /// not through `ReloadSignal` at all.
    Heartbeat,
}

/// The six config paths a write/edit tool call recognizes as
/// reload-triggering, canonicalized once so a written path can be compared
/// by equality rather than re-resolving symlinks/`..`/relative forms on
/// every write.
#[derive(Clone)]
pub struct RecognizedConfigPaths {
    // (canonicalized path, the reload pathway it funnels into)
    paths: Vec<(PathBuf, ConfigReloadKind)>,
}

impl RecognizedConfigPaths {
    /// Build the recognized set from the config directory (`config.toml`,
    /// `providers.toml`) and workspace layout (`mcp.json`, `channels.toml`,
    /// `a2a.json`, `HEARTBEAT.yml`).
    #[must_use]
    pub fn new(config_dir: &Path, layout: &WorkspaceLayout) -> Self {
        let canon = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
        Self {
            paths: vec![
                (
                    canon(config_dir.join("config.toml")),
                    ConfigReloadKind::Root,
                ),
                (
                    canon(config_dir.join("providers.toml")),
                    ConfigReloadKind::Root,
                ),
                (canon(layout.mcp_json()), ConfigReloadKind::Workspace),
                (canon(layout.channels_toml()), ConfigReloadKind::Workspace),
                (canon(layout.a2a_agents_json()), ConfigReloadKind::Workspace),
                (canon(layout.heartbeat_yml()), ConfigReloadKind::Heartbeat),
            ],
        }
    }

    /// Which reload pathway `written_path` funnels into, if it's one of the
    /// six recognized config paths. `written_path` should already exist (the
    /// write already succeeded), so it can be canonicalized for a robust
    /// comparison against symlinks/`..`/relative forms.
    #[must_use]
    pub fn kind_for(&self, written_path: &Path) -> Option<ConfigReloadKind> {
        let canonical = std::fs::canonicalize(written_path).ok()?;
        self.paths
            .iter()
            .find(|(p, _)| *p == canonical)
            .map(|(_, kind)| *kind)
    }
}

/// One write the agent made to a recognized config path, awaiting its
/// reload's outcome.
struct PendingWrite {
    kind: ConfigReloadKind,
    marked_at: Instant,
}

/// How long a pending mark stays valid before a matching reload is treated
/// as unrelated to it — e.g. a manual edit made through the web UI shortly
/// after the agent's own write, or a reload long after the write already
/// settled some other way.
const PENDING_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared between the agent's `write_file`/`edit_file` tools (which mark a
/// write) and the gateway event loop (which consumes the mark once the
/// matching reload completes, delivering its outcome to the agent).
#[derive(Clone, Default)]
pub struct SharedConfigReloadTracker(Arc<Mutex<Vec<PendingWrite>>>);

impl SharedConfigReloadTracker {
    /// Create a new, empty tracker.
    #[must_use]
    pub fn new_shared() -> Self {
        Self::default()
    }

    /// Record that the agent just wrote a path funneling into `kind`'s
    /// reload pathway.
    pub fn mark(&self, kind: ConfigReloadKind) {
        let mut pending = self.lock();
        pending.retain(|p| p.marked_at.elapsed() < PENDING_WRITE_TIMEOUT);
        pending.push(PendingWrite {
            kind,
            marked_at: Instant::now(),
        });
    }

    /// Consume the oldest still-valid pending mark for `kind`, if any.
    /// `true` means this reload was caused by the agent's own write and its
    /// outcome should also reach the agent, not just the user's interfaces.
    #[must_use]
    pub fn take_if_matches(&self, kind: ConfigReloadKind) -> bool {
        let mut pending = self.lock();
        pending.retain(|p| p.marked_at.elapsed() < PENDING_WRITE_TIMEOUT);
        if let Some(idx) = pending.iter().position(|p| p.kind == kind) {
            pending.remove(idx);
            true
        } else {
            false
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<PendingWrite>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What `write_file`/`edit_file` need to recognize and mark a config write —
/// bundles [`RecognizedConfigPaths`] and the [`SharedConfigReloadTracker`]
/// they mark on a match.
#[derive(Clone)]
pub struct ConfigWriteWatch {
    pub recognized: RecognizedConfigPaths,
    pub tracker: SharedConfigReloadTracker,
}

impl ConfigWriteWatch {
    /// Check whether `written_path` is a recognized config path and, if so,
    /// mark it on the shared tracker.
    pub fn note_write(&self, written_path: &Path) {
        if let Some(kind) = self.recognized.kind_for(written_path) {
            self.tracker.mark(kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_and_config_dir(dir: &Path) -> (WorkspaceLayout, PathBuf) {
        let layout = WorkspaceLayout::new(dir.join("workspace"));
        std::fs::create_dir_all(layout.root().join("config")).unwrap();
        let config_dir = dir.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        (layout, config_dir)
    }

    #[test]
    fn recognizes_config_toml_and_providers_toml_as_root() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, config_dir) = layout_and_config_dir(dir.path());
        std::fs::write(config_dir.join("config.toml"), "").unwrap();
        std::fs::write(config_dir.join("providers.toml"), "").unwrap();

        let recognized = RecognizedConfigPaths::new(&config_dir, &layout);
        assert_eq!(
            recognized.kind_for(&config_dir.join("config.toml")),
            Some(ConfigReloadKind::Root)
        );
        assert_eq!(
            recognized.kind_for(&config_dir.join("providers.toml")),
            Some(ConfigReloadKind::Root)
        );
    }

    #[test]
    fn recognizes_workspace_files_as_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, config_dir) = layout_and_config_dir(dir.path());
        std::fs::write(layout.mcp_json(), "").unwrap();
        std::fs::write(layout.channels_toml(), "").unwrap();
        std::fs::write(layout.a2a_agents_json(), "").unwrap();

        let recognized = RecognizedConfigPaths::new(&config_dir, &layout);
        assert_eq!(
            recognized.kind_for(&layout.mcp_json()),
            Some(ConfigReloadKind::Workspace)
        );
        assert_eq!(
            recognized.kind_for(&layout.channels_toml()),
            Some(ConfigReloadKind::Workspace)
        );
        assert_eq!(
            recognized.kind_for(&layout.a2a_agents_json()),
            Some(ConfigReloadKind::Workspace)
        );
    }

    #[test]
    fn recognizes_heartbeat_yml() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, config_dir) = layout_and_config_dir(dir.path());
        std::fs::write(layout.heartbeat_yml(), "").unwrap();

        let recognized = RecognizedConfigPaths::new(&config_dir, &layout);
        assert_eq!(
            recognized.kind_for(&layout.heartbeat_yml()),
            Some(ConfigReloadKind::Heartbeat)
        );
    }

    #[test]
    fn an_unrelated_path_matches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, config_dir) = layout_and_config_dir(dir.path());
        let other = layout.root().join("notes.md");
        std::fs::write(&other, "").unwrap();

        let recognized = RecognizedConfigPaths::new(&config_dir, &layout);
        assert_eq!(recognized.kind_for(&other), None);
    }

    #[test]
    fn a_write_marks_and_a_matching_reload_consumes_it_once() {
        let tracker = SharedConfigReloadTracker::new_shared();
        tracker.mark(ConfigReloadKind::Root);

        assert!(
            tracker.take_if_matches(ConfigReloadKind::Root),
            "the first matching reload should see the agent's own write"
        );
        assert!(
            !tracker.take_if_matches(ConfigReloadKind::Root),
            "the mark is consumed — a second reload isn't attributed to the same write"
        );
    }

    #[test]
    fn an_unrelated_reload_kind_does_not_consume_or_match() {
        let tracker = SharedConfigReloadTracker::new_shared();
        tracker.mark(ConfigReloadKind::Root);

        assert!(
            !tracker.take_if_matches(ConfigReloadKind::Workspace),
            "a workspace reload must not claim a root-config write"
        );
        assert!(
            tracker.take_if_matches(ConfigReloadKind::Root),
            "the root mark must still be there for the reload that actually matches"
        );
    }

    #[test]
    fn a_stale_mark_past_the_timeout_is_not_attributed_to_a_later_reload() {
        let tracker = SharedConfigReloadTracker::new_shared();
        {
            // Backdate the mark past the timeout window directly, rather
            // than sleeping the test for 30s.
            let mut pending = tracker.lock();
            pending.push(PendingWrite {
                kind: ConfigReloadKind::Root,
                marked_at: Instant::now().checked_sub(Duration::from_secs(31)).unwrap(),
            });
        }

        assert!(
            !tracker.take_if_matches(ConfigReloadKind::Root),
            "a mark older than the timeout must not be attributed to a later reload"
        );
    }

    #[test]
    fn note_write_marks_only_recognized_paths() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, config_dir) = layout_and_config_dir(dir.path());
        std::fs::write(config_dir.join("config.toml"), "").unwrap();
        let other = layout.root().join("notes.md");
        std::fs::write(&other, "").unwrap();

        let watch = ConfigWriteWatch {
            recognized: RecognizedConfigPaths::new(&config_dir, &layout),
            tracker: SharedConfigReloadTracker::new_shared(),
        };

        watch.note_write(&other);
        assert!(
            !watch.tracker.take_if_matches(ConfigReloadKind::Root),
            "an unrecognized path must never mark anything"
        );

        watch.note_write(&config_dir.join("config.toml"));
        assert!(
            watch.tracker.take_if_matches(ConfigReloadKind::Root),
            "a recognized path must mark its reload kind"
        );
    }
}
