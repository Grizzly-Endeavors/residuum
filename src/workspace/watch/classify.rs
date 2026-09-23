//! Maps one OS file notification to workspace-relative raw changes.

use std::path::{Component, Path, PathBuf};

use notify::EventKind;
use notify::event::{ModifyKind, RenameMode};

use super::batcher::{RawChange, RawOp};
use crate::workspace::access::is_blocked_path;

/// What one notification means for the workspace.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Classified {
    /// Paths the notification touched, minus hidden and outside paths.
    pub changes: Vec<RawChange>,
    /// The OS dropped notifications and the whole tree must be rescanned.
    pub events_lost: bool,
    /// The workspace root itself was removed or renamed away, so the watch
    /// on it is gone.
    pub root_gone: bool,
}

/// The workspace root, in each spelling the OS may report paths under: as
/// configured, and canonical (macOS reports `/private/var/...` for
/// `/var/...`).
#[derive(Debug, Clone)]
pub(super) struct WorkspaceRoots {
    spellings: Vec<PathBuf>,
}

impl WorkspaceRoots {
    pub fn new(root: &Path) -> Self {
        let mut spellings = vec![root.to_path_buf()];
        if let Ok(canonical) = std::fs::canonicalize(root)
            && canonical != root
        {
            spellings.push(canonical);
        }
        Self { spellings }
    }

    /// `path` relative to the workspace, `/`-separated. `Some("")` for the
    /// root itself; `None` outside the workspace or when the path is not
    /// valid UTF-8.
    pub fn relative(&self, path: &Path) -> Option<String> {
        let rest = self
            .spellings
            .iter()
            .find_map(|root| path.strip_prefix(root).ok())?;
        let mut segments = Vec::new();
        for component in rest.components() {
            match component {
                Component::Normal(segment) => segments.push(segment.to_str()?),
                Component::CurDir => {}
                Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
            }
        }
        Some(segments.join("/"))
    }
}

/// Classify one notification.
pub(super) fn classify(event: &notify::Event, roots: &WorkspaceRoots) -> Classified {
    let mut classified = Classified {
        events_lost: event.need_rescan(),
        ..Classified::default()
    };
    let ops: Vec<(&PathBuf, RawOp)> = match event.kind {
        // Reads (and the open/close around writes, which also report a
        // data change) change nothing. Ignoring them keeps an artifact that
        // reads a file on each change from feeding its own loop.
        EventKind::Access(_) | EventKind::Other => Vec::new(),
        EventKind::Create(_) => with_op(&event.paths, RawOp::Appeared),
        EventKind::Remove(_) => with_op(&event.paths, RawOp::Vanished),
        EventKind::Modify(ModifyKind::Name(mode)) => match mode {
            RenameMode::From => with_op(&event.paths, RawOp::Vanished),
            RenameMode::To => with_op(&event.paths, RawOp::Appeared),
            RenameMode::Both => event
                .paths
                .iter()
                .zip([RawOp::Vanished, RawOp::Appeared])
                .collect(),
            RenameMode::Any | RenameMode::Other => with_op(&event.paths, RawOp::Renamed),
        },
        EventKind::Modify(_) | EventKind::Any => with_op(&event.paths, RawOp::Changed),
    };

    for (path, op) in ops {
        let Some(relative) = roots.relative(path) else {
            continue;
        };
        if relative.is_empty() {
            if matches!(op, RawOp::Vanished | RawOp::Renamed) && !path.exists() {
                classified.root_gone = true;
            }
            continue;
        }
        if is_blocked_path(&relative) {
            continue;
        }
        classified.changes.push(RawChange { path: relative, op });
    }
    classified
}

fn with_op(paths: &[PathBuf], op: RawOp) -> Vec<(&PathBuf, RawOp)> {
    paths.iter().map(|p| (p, op)).collect()
}

#[cfg(test)]
mod tests {
    use notify::event::{AccessKind, CreateKind, DataChange, Flag, RemoveKind};

    use super::*;

    fn roots() -> (tempfile::TempDir, WorkspaceRoots) {
        let dir = tempfile::tempdir().unwrap();
        let roots = WorkspaceRoots::new(dir.path());
        (dir, roots)
    }

    fn event(kind: EventKind, paths: &[PathBuf]) -> notify::Event {
        let mut event = notify::Event::new(kind);
        for path in paths {
            event = event.add_path(path.clone());
        }
        event
    }

    fn raw(path: &str, op: RawOp) -> RawChange {
        RawChange {
            path: path.to_string(),
            op,
        }
    }

    #[test]
    fn relative_paths_are_slash_separated_and_confined_to_the_workspace() {
        let (dir, roots) = roots();
        assert_eq!(
            roots.relative(&dir.path().join("wiki").join("a.md")),
            Some("wiki/a.md".to_string())
        );
        assert_eq!(roots.relative(dir.path()), Some(String::new()));
        assert_eq!(roots.relative(Path::new("/elsewhere/a.md")), None);
    }

    #[test]
    fn canonical_spellings_of_the_root_are_recognized() {
        let (dir, roots) = roots();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            roots.relative(&canonical.join("a.md")),
            Some("a.md".to_string())
        );
    }

    #[test]
    fn creates_removes_and_writes_map_to_raw_ops() {
        let (dir, roots) = roots();
        let a = dir.path().join("a.md");
        let create = classify(
            &event(
                EventKind::Create(CreateKind::File),
                std::slice::from_ref(&a),
            ),
            &roots,
        );
        assert_eq!(create.changes, [raw("a.md", RawOp::Appeared)]);
        let write = classify(
            &event(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                std::slice::from_ref(&a),
            ),
            &roots,
        );
        assert_eq!(write.changes, [raw("a.md", RawOp::Changed)]);
        let remove = classify(&event(EventKind::Remove(RemoveKind::File), &[a]), &roots);
        assert_eq!(remove.changes, [raw("a.md", RawOp::Vanished)]);
    }

    #[test]
    fn a_paired_rename_is_a_vanish_and_an_appear() {
        let (dir, roots) = roots();
        let both = classify(
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &[dir.path().join("old.md"), dir.path().join("new.md")],
            ),
            &roots,
        );
        assert_eq!(
            both.changes,
            [
                raw("old.md", RawOp::Vanished),
                raw("new.md", RawOp::Appeared)
            ]
        );
    }

    #[test]
    fn reads_are_ignored() {
        let (dir, roots) = roots();
        let open = classify(
            &event(
                EventKind::Access(AccessKind::Any),
                &[dir.path().join("a.md")],
            ),
            &roots,
        );
        assert_eq!(open, Classified::default());
    }

    #[test]
    fn blocked_paths_never_appear() {
        let (dir, roots) = roots();
        let paths = [
            dir.path().join("memory/.index/seg.bin"),
            dir.path().join("memory/vectors.db"),
            dir.path().join("memory/vectors.db-wal"),
            dir.path().join("wiki/.a.md.0badf00d.residuum-tmp"),
            dir.path().join("wiki/a.md"),
        ];
        let classified = classify(&event(EventKind::Create(CreateKind::Any), &paths), &roots);
        assert_eq!(classified.changes, [raw("wiki/a.md", RawOp::Appeared)]);
    }

    #[test]
    fn a_rescan_flag_marks_events_lost() {
        let (_dir, roots) = roots();
        let rescan = notify::Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert!(classify(&rescan, &roots).events_lost);
    }

    #[test]
    fn removing_the_root_is_reported_once_it_is_really_gone() {
        let (dir, roots) = roots();
        let root = dir.path().to_path_buf();
        let removed = event(
            EventKind::Remove(RemoveKind::Folder),
            std::slice::from_ref(&root),
        );
        assert!(
            !classify(&removed, &roots).root_gone,
            "the root still exists"
        );
        drop(dir);
        let classified = classify(&removed, &roots);
        assert!(classified.root_gone);
        assert!(classified.changes.is_empty());
    }
}
