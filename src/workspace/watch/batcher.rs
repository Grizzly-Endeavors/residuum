//! Debouncing: raw notifications coalesce into one batch until the workspace
//! has been quiet for [`QUIET_PERIOD`], or [`MAX_BATCH_DELAY`] after the
//! batch's first notification while writes continue.
//!
//! Kept free of the OS watcher and the filesystem: the caller passes the
//! current time in, and resolving a batch takes a function that reports what
//! is at a path now, so timing and resolution are testable on their own.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use super::{WorkspaceChange, WorkspaceChangeKind};

/// A batch is flushed once no notification has arrived for this long.
pub(super) const QUIET_PERIOD: Duration = Duration::from_millis(300);

/// A batch is flushed this long after its first notification, even while
/// notifications keep arriving.
pub(super) const MAX_BATCH_DELAY: Duration = Duration::from_secs(2);

/// A burst touching more distinct paths than this becomes a resync: listing
/// them all would cost more than every watcher reloading.
pub(super) const MAX_PENDING_PATHS: usize = 10_000;

/// How one raw notification touched a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RawOp {
    /// Created, or renamed into place.
    Appeared,
    /// Deleted, or renamed away.
    Vanished,
    /// Renamed, without saying which side (macOS reports renames this way).
    Renamed,
    /// Content or metadata changed.
    Changed,
}

/// One workspace-relative path a notification touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawChange {
    pub path: String,
    pub op: RawOp,
}

/// What is at a workspace path when a batch is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PathState {
    Missing,
    File,
    Directory,
}

/// Everything a batch saw for one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PathHistory {
    /// The batch's first operation on the path, which says whether the path
    /// existed before the batch.
    first: RawOp,
    /// Whether every operation only changed content or metadata.
    only_changed: bool,
}

/// A batch ready to resolve and publish.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReadyBatch {
    /// Notifications were lost: watchers must resync, and the batch's own
    /// changes are dropped.
    EventsLost,
    /// The touched paths, sorted.
    Paths(Vec<(String, PathHistory)>),
}

/// Accumulates raw changes until the batch is due.
#[derive(Debug, Default)]
pub(super) struct ChangeBatcher {
    pending: HashMap<String, PathHistory>,
    events_lost: bool,
    first_at: Option<Instant>,
    last_at: Option<Instant>,
}

impl ChangeBatcher {
    /// Record a raw change seen at `now`.
    pub fn record(&mut self, change: RawChange, now: Instant) {
        self.touch(now);
        if self.events_lost {
            return;
        }
        let changed = change.op == RawOp::Changed;
        self.pending
            .entry(change.path)
            .and_modify(|history| history.only_changed &= changed)
            .or_insert(PathHistory {
                first: change.op,
                only_changed: changed,
            });
        if self.pending.len() > MAX_PENDING_PATHS {
            self.record_events_lost(now);
        }
    }

    /// Record that notifications were lost (an OS queue overflow or rescan).
    pub fn record_events_lost(&mut self, now: Instant) {
        self.touch(now);
        self.events_lost = true;
        self.pending.clear();
    }

    fn touch(&mut self, now: Instant) {
        self.first_at.get_or_insert(now);
        self.last_at = Some(now);
    }

    /// When the pending batch is due, or `None` when nothing is pending.
    pub fn deadline(&self) -> Option<Instant> {
        let first = self.first_at?;
        let last = self.last_at.unwrap_or(first);
        Some((last + QUIET_PERIOD).min(first + MAX_BATCH_DELAY))
    }

    /// Take the pending batch and start a new one. `None` when nothing was
    /// recorded.
    pub fn take(&mut self) -> Option<ReadyBatch> {
        self.first_at.take()?;
        self.last_at = None;
        if std::mem::take(&mut self.events_lost) {
            self.pending.clear();
            return Some(ReadyBatch::EventsLost);
        }
        let mut paths: Vec<_> = self.pending.drain().collect();
        paths.sort_by(|a, b| a.0.cmp(&b.0));
        Some(ReadyBatch::Paths(paths))
    }
}

/// Resolve each touched path to one change, from what the batch saw and what
/// is at the path now. A directory whose only change was its own metadata
/// produces nothing: its children report the real changes.
pub(super) fn resolve_changes(
    paths: Vec<(String, PathHistory)>,
    state_of: impl Fn(&str) -> PathState,
) -> Vec<WorkspaceChange> {
    paths
        .into_iter()
        .filter_map(|(path, history)| {
            let kind = resolve_kind(history, state_of(&path))?;
            Some(WorkspaceChange { path, kind })
        })
        .collect()
}

fn resolve_kind(history: PathHistory, now: PathState) -> Option<WorkspaceChangeKind> {
    match now {
        // Even a path that appeared and vanished within the batch reports
        // `removed`: macOS can replay a path's creation alongside its later
        // rename, and dropping a real removal would leave a watcher stale,
        // while a removal of a path it never saw costs it nothing.
        PathState::Missing => Some(WorkspaceChangeKind::Removed),
        PathState::Directory if history.only_changed => None,
        PathState::File | PathState::Directory => Some(match history.first {
            RawOp::Appeared | RawOp::Renamed => WorkspaceChangeKind::Created,
            RawOp::Vanished | RawOp::Changed => WorkspaceChangeKind::Modified,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(path: &str, op: RawOp) -> RawChange {
        RawChange {
            path: path.to_string(),
            op,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn resolve_all(batch: ReadyBatch, states: &[(&str, PathState)]) -> Vec<WorkspaceChange> {
        let ReadyBatch::Paths(paths) = batch else {
            panic!("expected paths, got {batch:?}");
        };
        let states: HashMap<String, PathState> =
            states.iter().map(|(p, s)| ((*p).to_string(), *s)).collect();
        resolve_changes(paths, |p| {
            states.get(p).copied().unwrap_or(PathState::Missing)
        })
    }

    fn change(path: &str, kind: WorkspaceChangeKind) -> WorkspaceChange {
        WorkspaceChange {
            path: path.to_string(),
            kind,
        }
    }

    #[test]
    fn nothing_pending_has_no_deadline_and_no_batch() {
        let mut batcher = ChangeBatcher::default();
        assert_eq!(batcher.deadline(), None);
        assert_eq!(batcher.take(), None);
    }

    #[test]
    fn a_burst_of_writes_becomes_one_batch_after_the_quiet_period() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        for i in 0..20_u64 {
            batcher.record(
                raw(&format!("wiki/p{i:02}.md"), RawOp::Appeared),
                t0 + ms(i * 10),
            );
            batcher.record(
                raw(&format!("wiki/p{i:02}.md"), RawOp::Changed),
                t0 + ms(i * 10),
            );
        }
        // Quiet period runs from the last write, well before the 2 s cap.
        assert_eq!(batcher.deadline(), Some(t0 + ms(190) + QUIET_PERIOD));

        let batch = batcher.take().unwrap();
        let states: Vec<(String, PathState)> = (0..20)
            .map(|i| (format!("wiki/p{i:02}.md"), PathState::File))
            .collect();
        let states: Vec<(&str, PathState)> = states.iter().map(|(p, s)| (p.as_str(), *s)).collect();
        let changes = resolve_all(batch, &states);
        assert_eq!(changes.len(), 20);
        assert!(
            changes
                .iter()
                .all(|c| c.kind == WorkspaceChangeKind::Created)
        );
        assert_eq!(changes.first().unwrap().path, "wiki/p00.md");
        assert_eq!(batcher.take(), None, "taking a batch starts a new one");
    }

    #[test]
    fn continuous_writes_flush_two_seconds_after_the_first() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        for i in 0..50_u64 {
            batcher.record(raw("log.md", RawOp::Changed), t0 + ms(i * 100));
        }
        assert_eq!(batcher.deadline(), Some(t0 + MAX_BATCH_DELAY));
    }

    #[test]
    fn a_rename_becomes_removed_and_created() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        batcher.record(raw("wiki/old.md", RawOp::Vanished), t0);
        batcher.record(raw("wiki/new.md", RawOp::Appeared), t0);
        let changes = resolve_all(batcher.take().unwrap(), &[("wiki/new.md", PathState::File)]);
        assert_eq!(
            changes,
            [
                change("wiki/new.md", WorkspaceChangeKind::Created),
                change("wiki/old.md", WorkspaceChangeKind::Removed),
            ]
        );
    }

    #[test]
    fn an_unpaired_rename_resolves_from_what_exists_now() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        batcher.record(raw("a.md", RawOp::Renamed), t0);
        batcher.record(raw("b.md", RawOp::Renamed), t0);
        let changes = resolve_all(batcher.take().unwrap(), &[("b.md", PathState::File)]);
        assert_eq!(
            changes,
            [
                change("a.md", WorkspaceChangeKind::Removed),
                change("b.md", WorkspaceChangeKind::Created),
            ]
        );
    }

    #[test]
    fn a_path_missing_at_the_end_of_a_batch_is_removed() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        batcher.record(raw("scratch.txt", RawOp::Appeared), t0);
        batcher.record(raw("scratch.txt", RawOp::Changed), t0);
        batcher.record(raw("scratch.txt", RawOp::Vanished), t0);
        assert_eq!(
            resolve_all(batcher.take().unwrap(), &[]),
            [change("scratch.txt", WorkspaceChangeKind::Removed)]
        );
    }

    #[test]
    fn a_file_deleted_and_written_again_is_modified() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        batcher.record(raw("note.md", RawOp::Vanished), t0);
        batcher.record(raw("note.md", RawOp::Appeared), t0);
        let changes = resolve_all(batcher.take().unwrap(), &[("note.md", PathState::File)]);
        assert_eq!(changes, [change("note.md", WorkspaceChangeKind::Modified)]);
    }

    #[test]
    fn a_directory_metadata_change_alone_is_dropped() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        batcher.record(raw("wiki", RawOp::Changed), t0);
        batcher.record(raw("wiki/a.md", RawOp::Changed), t0);
        batcher.record(raw("fresh", RawOp::Appeared), t0);
        let changes = resolve_all(
            batcher.take().unwrap(),
            &[
                ("wiki", PathState::Directory),
                ("wiki/a.md", PathState::File),
                ("fresh", PathState::Directory),
            ],
        );
        assert_eq!(
            changes,
            [
                change("fresh", WorkspaceChangeKind::Created),
                change("wiki/a.md", WorkspaceChangeKind::Modified),
            ]
        );
    }

    #[test]
    fn lost_events_replace_the_batch_with_a_resync() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        batcher.record(raw("a.md", RawOp::Changed), t0);
        batcher.record_events_lost(t0 + ms(10));
        batcher.record(raw("b.md", RawOp::Changed), t0 + ms(20));
        assert_eq!(batcher.deadline(), Some(t0 + ms(20) + QUIET_PERIOD));
        assert_eq!(batcher.take(), Some(ReadyBatch::EventsLost));
        batcher.record(raw("c.md", RawOp::Changed), t0 + ms(400));
        assert!(
            matches!(batcher.take(), Some(ReadyBatch::Paths(p)) if p.len() == 1),
            "the next batch lists changes again"
        );
    }

    #[test]
    fn a_burst_past_the_pending_limit_becomes_a_resync() {
        let t0 = Instant::now();
        let mut batcher = ChangeBatcher::default();
        for i in 0..=MAX_PENDING_PATHS {
            batcher.record(raw(&format!("f{i}"), RawOp::Appeared), t0);
        }
        assert_eq!(batcher.take(), Some(ReadyBatch::EventsLost));
    }
}
