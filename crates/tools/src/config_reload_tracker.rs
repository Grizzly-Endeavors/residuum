use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use residuum_config::WorkspaceLayout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigReloadKind {
    Agent,
    Hub,
    Workspace,
    Heartbeat,
}

#[derive(Clone)]
pub struct RecognizedConfigPaths {
    paths: Vec<(PathBuf, ConfigReloadKind)>,
}

impl RecognizedConfigPaths {
    #[must_use]
    pub fn new(config_dir: &Path, hub_dir: &Path, layout: &WorkspaceLayout) -> Self {
        let canon = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
        Self {
            paths: vec![
                (
                    canon(config_dir.join("config.toml")),
                    ConfigReloadKind::Agent,
                ),
                (
                    canon(config_dir.join("providers.toml")),
                    ConfigReloadKind::Agent,
                ),
                (canon(hub_dir.join("config.toml")), ConfigReloadKind::Hub),
                (canon(layout.mcp_json()), ConfigReloadKind::Workspace),
                (canon(layout.channels_toml()), ConfigReloadKind::Workspace),
                (canon(layout.a2a_agents_json()), ConfigReloadKind::Workspace),
                (canon(layout.heartbeat_yml()), ConfigReloadKind::Heartbeat),
            ],
        }
    }

    #[must_use]
    pub fn kind_for(&self, written_path: &Path) -> Option<ConfigReloadKind> {
        let canonical = std::fs::canonicalize(written_path).ok()?;
        self.paths
            .iter()
            .find(|(p, _)| *p == canonical)
            .map(|(_, kind)| *kind)
    }
}

struct PendingWrite {
    kind: ConfigReloadKind,
    marked_at: Instant,
}

const PENDING_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub struct SharedConfigReloadTracker(Arc<Mutex<Vec<PendingWrite>>>);

impl SharedConfigReloadTracker {
    #[must_use]
    pub fn new_shared() -> Self {
        Self::default()
    }

    pub fn mark(&self, kind: ConfigReloadKind) {
        let mut pending = self.lock();
        pending.retain(|p| p.marked_at.elapsed() < PENDING_WRITE_TIMEOUT);
        pending.push(PendingWrite {
            kind,
            marked_at: Instant::now(),
        });
    }

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

#[derive(Clone)]
pub struct ConfigWriteWatch {
    pub recognized: RecognizedConfigPaths,
    pub tracker: SharedConfigReloadTracker,
}

impl ConfigWriteWatch {
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

        let recognized = RecognizedConfigPaths::new(&config_dir, &config_dir.join("hub"), &layout);
        assert_eq!(
            recognized.kind_for(&config_dir.join("config.toml")),
            Some(ConfigReloadKind::Agent)
        );
        assert_eq!(
            recognized.kind_for(&config_dir.join("providers.toml")),
            Some(ConfigReloadKind::Agent)
        );
    }

    #[test]
    fn recognizes_the_hub_config_as_hub() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, config_dir) = layout_and_config_dir(dir.path());
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "").unwrap();

        let recognized = RecognizedConfigPaths::new(&config_dir, &hub_dir, &layout);
        assert_eq!(
            recognized.kind_for(&hub_dir.join("config.toml")),
            Some(ConfigReloadKind::Hub)
        );
    }

    #[test]
    fn recognizes_workspace_files_as_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, config_dir) = layout_and_config_dir(dir.path());
        std::fs::write(layout.mcp_json(), "").unwrap();
        std::fs::write(layout.channels_toml(), "").unwrap();
        std::fs::write(layout.a2a_agents_json(), "").unwrap();

        let recognized = RecognizedConfigPaths::new(&config_dir, &config_dir.join("hub"), &layout);
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

        let recognized = RecognizedConfigPaths::new(&config_dir, &config_dir.join("hub"), &layout);
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

        let recognized = RecognizedConfigPaths::new(&config_dir, &config_dir.join("hub"), &layout);
        assert_eq!(recognized.kind_for(&other), None);
    }

    #[test]
    fn a_write_marks_and_a_matching_reload_consumes_it_once() {
        let tracker = SharedConfigReloadTracker::new_shared();
        tracker.mark(ConfigReloadKind::Agent);

        assert!(
            tracker.take_if_matches(ConfigReloadKind::Agent),
            "the first matching reload should see the agent's own write"
        );
        assert!(
            !tracker.take_if_matches(ConfigReloadKind::Agent),
            "the mark is consumed — a second reload isn't attributed to the same write"
        );
    }

    #[test]
    fn an_unrelated_reload_kind_does_not_consume_or_match() {
        let tracker = SharedConfigReloadTracker::new_shared();
        tracker.mark(ConfigReloadKind::Agent);

        assert!(
            !tracker.take_if_matches(ConfigReloadKind::Workspace),
            "a workspace reload must not claim a root-config write"
        );
        assert!(
            tracker.take_if_matches(ConfigReloadKind::Agent),
            "the root mark must still be there for the reload that actually matches"
        );
    }

    #[test]
    fn a_stale_mark_past_the_timeout_is_not_attributed_to_a_later_reload() {
        let tracker = SharedConfigReloadTracker::new_shared();
        {
            let mut pending = tracker.lock();
            pending.push(PendingWrite {
                kind: ConfigReloadKind::Agent,
                marked_at: Instant::now().checked_sub(Duration::from_secs(31)).unwrap(),
            });
        }

        assert!(
            !tracker.take_if_matches(ConfigReloadKind::Agent),
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
            recognized: RecognizedConfigPaths::new(&config_dir, &config_dir.join("hub"), &layout),
            tracker: SharedConfigReloadTracker::new_shared(),
        };

        watch.note_write(&other);
        assert!(
            !watch.tracker.take_if_matches(ConfigReloadKind::Agent),
            "an unrecognized path must never mark anything"
        );

        watch.note_write(&config_dir.join("config.toml"));
        assert!(
            watch.tracker.take_if_matches(ConfigReloadKind::Agent),
            "a recognized path must mark its reload kind"
        );
    }
}
