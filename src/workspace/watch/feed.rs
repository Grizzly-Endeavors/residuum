//! Runs the OS watcher over the workspace and turns its notifications into
//! published batches.
//!
//! Native notifications are tried first; when they can't start (the Linux
//! watch limit, an unusual filesystem) the feed falls back to notify's polling
//! watcher. When neither starts, live updates are off: the feed logs an error,
//! reports [`WatchHealth::Off`], and publishes [`WorkspaceEvent::Unavailable`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use notify::{Config, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::batcher::{ChangeBatcher, PathState, ReadyBatch, resolve_changes};
use super::classify::{WorkspaceRoots, classify};
use super::{WatchHealth, WorkspaceChange, WorkspaceResyncReason};
use crate::bus::{Publisher, WorkspaceEvent, topics};

/// Raw notifications buffered between the OS watcher's thread and the
/// batching loop. Past this, notifications are dropped and the next batch
/// becomes a resync.
const RAW_EVENT_CAPACITY: usize = 4096;

/// How often the polling fallback rescans the workspace.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Repeated watcher errors and lost-event resyncs are logged at `warn` at
/// most this often; the ones in between are counted into the next line.
const WARN_INTERVAL: Duration = Duration::from_secs(60);

type RawEvent = notify::Result<notify::Event>;

/// Start a change feed over `root`. Runs until aborted.
///
/// With a `prefix`, `root` is a directory that appears in the workspace
/// namespace under that prefix (the team directory as `team`): published
/// paths carry it, and the access policy is applied to the prefixed path.
pub(crate) fn spawn_change_feed(
    root: PathBuf,
    prefix: Option<&str>,
    publisher: Publisher,
    health: watch::Sender<WatchHealth>,
) -> JoinHandle<()> {
    let prefix = prefix.map(str::to_string);
    crate::util::spawn_in_span(async move {
        let (raw_tx, raw_rx) = mpsc::channel(RAW_EVENT_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let sink = RawSink {
            tx: raw_tx,
            overflowed: Arc::clone(&overflowed),
            roots: WorkspaceRoots::new(&root, prefix.as_deref()),
        };

        let Some(mut backend) = start_backend(&root, Mode::Native, &sink).await else {
            live_updates_off(&publisher, &health).await;
            return std::future::pending().await;
        };
        health.send_replace(backend.health());

        let mut feed = FeedLoop::new(
            &root,
            prefix.as_deref(),
            raw_rx,
            overflowed,
            publisher.clone(),
        );
        loop {
            let restart_mode = match feed.run(backend.mode()).await {
                LoopExit::PublishFailed => return,
                LoopExit::Restart(mode) => mode,
            };
            // Release the old watches before placing new ones.
            drop(backend);
            feed.discard_pending();
            let Some(restarted) = start_backend(&root, restart_mode, &sink).await else {
                live_updates_off(&publisher, &health).await;
                return std::future::pending().await;
            };
            backend = restarted;
            health.send_replace(backend.health());
            tracing::info!(watcher = ?backend.health(), "workspace watcher restarted; watchers were told to resync");
            let resync = WorkspaceEvent::Resync(WorkspaceResyncReason::WatcherRestarted);
            if !feed.publish(resync).await {
                return;
            }
        }
    })
}

/// Report that no watcher is running.
async fn live_updates_off(publisher: &Publisher, health: &watch::Sender<WatchHealth>) {
    health.send_replace(WatchHealth::Off);
    if let Err(e) = publisher
        .publish(topics::Workspace, WorkspaceEvent::Unavailable)
        .await
    {
        tracing::warn!(error = %e, "failed to announce that workspace live updates are off");
    }
}

/// Which kind of OS watcher to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Native,
    Polling,
}

/// The running OS watcher.
struct Backend {
    mode: Mode,
    /// Held only to keep the watch alive: dropping it stops the watch.
    _watcher: Box<dyn Watcher + Send>,
}

impl Backend {
    fn mode(&self) -> Mode {
        self.mode
    }

    fn health(&self) -> WatchHealth {
        match self.mode {
            Mode::Native => WatchHealth::Native,
            Mode::Polling => WatchHealth::Polling,
        }
    }
}

/// Start a watcher over `root`, preferring `mode` and falling back from native
/// to polling. `None` when no watcher could start (already logged).
async fn start_backend(root: &Path, mode: Mode, sink: &RawSink) -> Option<Backend> {
    if mode == Mode::Native {
        let (root_owned, sink) = (root.to_path_buf(), sink.clone());
        match crate::util::spawn_blocking_in_span(move || start_native(&root_owned, sink)).await {
            Ok(Ok(watcher)) => {
                return Some(Backend {
                    mode: Mode::Native,
                    _watcher: Box::new(watcher),
                });
            }
            Ok(Err(e)) => tracing::warn!(
                error = %e,
                root = %root.display(),
                "native file notifications couldn't start; watching the workspace by polling instead"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                root = %root.display(),
                "starting native file notifications failed; watching the workspace by polling instead"
            ),
        }
    }
    let (root_owned, sink) = (root.to_path_buf(), sink.clone());
    match crate::util::spawn_blocking_in_span(move || start_polling(&root_owned, sink)).await {
        Ok(Ok(watcher)) => Some(Backend {
            mode: Mode::Polling,
            _watcher: Box::new(watcher),
        }),
        Ok(Err(e)) => {
            tracing::error!(error = %e, root = %root.display(), "couldn't watch the workspace for changes; live updates are off");
            None
        }
        Err(e) => {
            tracing::error!(error = %e, root = %root.display(), "starting the workspace polling watcher failed; live updates are off");
            None
        }
    }
}

fn start_native(root: &Path, sink: RawSink) -> notify::Result<RecommendedWatcher> {
    // Symlinks are not followed: a link to a folder outside the workspace
    // must not pull that folder into the feed.
    let mut watcher = RecommendedWatcher::new(sink, Config::default().with_follow_symlinks(false))?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

fn start_polling(root: &Path, sink: RawSink) -> notify::Result<PollWatcher> {
    let config = Config::default()
        .with_poll_interval(POLL_INTERVAL)
        .with_follow_symlinks(false);
    let mut watcher = PollWatcher::new(sink, config)?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// Hands OS notifications from the watcher's own thread to the batching
/// loop without ever blocking that thread.
#[derive(Clone)]
struct RawSink {
    tx: mpsc::Sender<RawEvent>,
    overflowed: Arc<AtomicBool>,
    roots: WorkspaceRoots,
}

impl RawSink {
    /// Whether `event` only touches paths the access policy hides. Residuum
    /// writes its memory index and databases constantly; dropping those
    /// notifications here keeps that churn from filling the buffer and
    /// forcing resyncs on watchers that never see those paths anyway.
    fn only_hidden_paths(&self, event: &notify::Event) -> bool {
        !event.need_rescan()
            && !event.paths.is_empty()
            && event.paths.iter().all(|path| {
                self.roots
                    .relative(path)
                    .is_some_and(|relative| self.roots.is_hidden(&relative))
            })
    }
}

impl notify::EventHandler for RawSink {
    fn handle_event(&mut self, event: RawEvent) {
        if event.as_ref().is_ok_and(|e| self.only_hidden_paths(e)) {
            return;
        }
        match self.tx.try_send(event) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.overflowed.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Why [`FeedLoop::run`] returned.
#[derive(Debug, PartialEq, Eq)]
enum LoopExit {
    /// Restart the OS watcher in this mode.
    Restart(Mode),
    /// The bus is gone; the feed stops.
    PublishFailed,
}

/// The batching loop: receives raw notifications, debounces them, and
/// publishes each batch.
struct FeedLoop {
    roots: WorkspaceRoots,
    root: PathBuf,
    raw_rx: mpsc::Receiver<RawEvent>,
    overflowed: Arc<AtomicBool>,
    publisher: Publisher,
    batcher: ChangeBatcher,
    error_warnings: ThrottledWarning,
    overflow_warnings: ThrottledWarning,
}

impl FeedLoop {
    fn new(
        root: &Path,
        prefix: Option<&str>,
        raw_rx: mpsc::Receiver<RawEvent>,
        overflowed: Arc<AtomicBool>,
        publisher: Publisher,
    ) -> Self {
        Self {
            roots: WorkspaceRoots::new(root, prefix),
            root: root.to_path_buf(),
            raw_rx,
            overflowed,
            publisher,
            batcher: ChangeBatcher::default(),
            error_warnings: ThrottledWarning::default(),
            overflow_warnings: ThrottledWarning::default(),
        }
    }

    /// Process notifications from a watcher running in `mode` until it has
    /// to be restarted or the bus is gone.
    async fn run(&mut self, mode: Mode) -> LoopExit {
        loop {
            let deadline = self.batcher.deadline();
            tokio::select! {
                raw = self.raw_rx.recv() => {
                    let Some(raw) = raw else {
                        tracing::warn!("workspace watcher stopped delivering notifications; restarting it");
                        return LoopExit::Restart(mode);
                    };
                    if let Some(exit) = self.handle_raw(raw, mode) {
                        return exit;
                    }
                }
                () = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
                    if !self.flush().await {
                        return LoopExit::PublishFailed;
                    }
                }
            }
        }
    }

    fn handle_raw(&mut self, raw: RawEvent, mode: Mode) -> Option<LoopExit> {
        let now = Instant::now();
        let exit = match raw {
            Ok(event) => {
                let classified = classify(&event, &self.roots);
                if classified.events_lost {
                    self.batcher.record_events_lost(now);
                }
                for change in classified.changes {
                    self.batcher.record(change, now);
                }
                classified.root_gone.then(|| {
                    tracing::error!(root = %self.root.display(), "the workspace folder was removed or renamed; restarting the workspace watcher");
                    LoopExit::Restart(Mode::Native)
                })
            }
            Err(e)
                if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) && mode == Mode::Native =>
            {
                tracing::warn!(error = %e, root = %self.root.display(), "the OS file watch limit was reached; watching the workspace by polling instead");
                Some(LoopExit::Restart(Mode::Polling))
            }
            Err(e) => {
                if let Some(suppressed) = self.error_warnings.should_warn(now) {
                    tracing::warn!(error = %e, paths = ?e.paths, suppressed, "workspace watcher reported an error");
                } else {
                    tracing::debug!(error = %e, paths = ?e.paths, "workspace watcher reported an error");
                }
                None
            }
        };
        if self.overflowed.swap(false, Ordering::Relaxed) {
            self.batcher.record_events_lost(now);
        }
        exit
    }

    /// Drop the pending batch; a restart's resync covers it.
    fn discard_pending(&mut self) {
        self.batcher = ChangeBatcher::default();
        self.overflowed.store(false, Ordering::Relaxed);
    }

    /// Publish the pending batch, if any. `false` when the bus is gone.
    async fn flush(&mut self) -> bool {
        let Some(batch) = self.batcher.take() else {
            return true;
        };
        let event = match batch {
            ReadyBatch::EventsLost => {
                if let Some(suppressed) = self.overflow_warnings.should_warn(Instant::now()) {
                    tracing::warn!(
                        suppressed,
                        "workspace change notifications were lost; watchers were told to resync"
                    );
                }
                WorkspaceEvent::Resync(WorkspaceResyncReason::Overflow)
            }
            ReadyBatch::Paths(paths) => {
                let root = self.root.clone();
                match crate::util::spawn_blocking_in_span(move || {
                    resolve_changes(paths, |path| path_state(&root, path))
                })
                .await
                {
                    Ok(changes) if changes.is_empty() => return true,
                    Ok(changes) => WorkspaceEvent::Changed(self.namespaced(changes).into()),
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to resolve a batch of workspace changes; watchers were told to resync");
                        WorkspaceEvent::Resync(WorkspaceResyncReason::Overflow)
                    }
                }
            }
        };
        self.publish(event).await
    }

    /// `changes` with their paths under the feed's namespace prefix.
    fn namespaced(&self, mut changes: Vec<WorkspaceChange>) -> Vec<WorkspaceChange> {
        for change in &mut changes {
            change.path = self.roots.namespaced(&change.path);
        }
        changes
    }

    /// Publish one event. `false` when the bus is gone.
    async fn publish(&self, event: WorkspaceEvent) -> bool {
        match self.publisher.publish(topics::Workspace, event).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "failed to publish workspace changes; stopping the workspace change feed");
                false
            }
        }
    }
}

/// What is at `relative` inside `root` right now. Symlinks count as files:
/// the feed reports the link, never what it points to.
fn path_state(root: &Path, relative: &str) -> PathState {
    match std::fs::symlink_metadata(root.join(relative)) {
        Ok(metadata) if metadata.is_dir() => PathState::Directory,
        Ok(_) => PathState::File,
        Err(_) => PathState::Missing,
    }
}

/// Lets one `warn` through per [`WARN_INTERVAL`] and counts the rest.
#[derive(Debug, Default)]
struct ThrottledWarning {
    last_warned: Option<Instant>,
    suppressed: u32,
}

impl ThrottledWarning {
    /// `Some(suppressed since the last warning)` when a warning may be
    /// logged now.
    fn should_warn(&mut self, now: Instant) -> Option<u32> {
        if self
            .last_warned
            .is_some_and(|last| now.duration_since(last) < WARN_INTERVAL)
        {
            self.suppressed = self.suppressed.saturating_add(1);
            return None;
        }
        self.last_warned = Some(now);
        Some(std::mem::take(&mut self.suppressed))
    }
}

#[cfg(test)]
mod tests {
    use notify::EventKind;
    use notify::event::{CreateKind, ModifyKind, RenameMode};

    use super::*;
    use crate::bus::Subscriber;
    use crate::workspace::watch::{WorkspaceChange, WorkspaceChangeKind};

    fn change(path: &str, kind: WorkspaceChangeKind) -> WorkspaceChange {
        WorkspaceChange {
            path: path.to_string(),
            kind,
        }
    }

    async fn next_event(sub: &mut Subscriber<WorkspaceEvent>) -> WorkspaceEvent {
        crate::testing::wait::guarded("a workspace event", sub.recv())
            .await
            .unwrap()
            .unwrap()
    }

    /// A feed loop fed by hand instead of an OS watcher.
    struct Harness {
        dir: tempfile::TempDir,
        raw_tx: mpsc::Sender<RawEvent>,
        overflowed: Arc<AtomicBool>,
        sub: Subscriber<WorkspaceEvent>,
        task: JoinHandle<LoopExit>,
    }

    async fn harness() -> Harness {
        harness_with_prefix(None).await
    }

    async fn harness_with_prefix(prefix: Option<&str>) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let sub = bus.subscribe(topics::Workspace).await.unwrap();
        let (raw_tx, raw_rx) = mpsc::channel(RAW_EVENT_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let mut feed = FeedLoop::new(
            dir.path(),
            prefix,
            raw_rx,
            Arc::clone(&overflowed),
            bus.publisher(),
        );
        let task = crate::util::spawn_in_span(async move { feed.run(Mode::Native).await });
        Harness {
            dir,
            raw_tx,
            overflowed,
            sub,
            task,
        }
    }

    impl Harness {
        async fn notify(&self, kind: EventKind, relative: &[&str]) {
            let mut event = notify::Event::new(kind);
            for path in relative {
                event = event.add_path(self.dir.path().join(path));
            }
            self.raw_tx.send(Ok(event)).await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_burst_of_notifications_is_published_as_one_sorted_batch() {
        let mut h = harness().await;
        std::fs::create_dir(h.dir.path().join("wiki")).unwrap();
        for i in 0..20 {
            let name = format!("wiki/p{i:02}.md");
            std::fs::write(h.dir.path().join(&name), "x").unwrap();
            h.notify(EventKind::Create(CreateKind::File), &[&name])
                .await;
            h.notify(EventKind::Modify(ModifyKind::Any), &[&name]).await;
        }
        let WorkspaceEvent::Changed(changes) = next_event(&mut h.sub).await else {
            panic!("expected a change batch");
        };
        let expected: Vec<_> = (0..20)
            .map(|i| change(&format!("wiki/p{i:02}.md"), WorkspaceChangeKind::Created))
            .collect();
        assert_eq!(&*changes, expected.as_slice());

        // Nothing else was published after the burst: the next batch is the one
        // a later write produces, so any stray batch from the burst would have
        // arrived before it.
        std::fs::write(h.dir.path().join("after.md"), "x").unwrap();
        h.notify(EventKind::Create(CreateKind::File), &["after.md"])
            .await;
        let WorkspaceEvent::Changed(after) = next_event(&mut h.sub).await else {
            panic!("expected a change batch");
        };
        assert_eq!(
            &*after,
            [change("after.md", WorkspaceChangeKind::Created)].as_slice(),
            "the burst produced a batch of its own after the first"
        );
        h.task.abort();
    }

    #[tokio::test]
    async fn a_rename_is_published_as_removed_and_created() {
        let mut h = harness().await;
        std::fs::write(h.dir.path().join("new.md"), "x").unwrap();
        h.notify(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["old.md", "new.md"],
        )
        .await;
        assert_eq!(
            next_event(&mut h.sub).await,
            WorkspaceEvent::Changed(
                vec![
                    change("new.md", WorkspaceChangeKind::Created),
                    change("old.md", WorkspaceChangeKind::Removed),
                ]
                .into()
            )
        );
        h.task.abort();
    }

    #[tokio::test]
    async fn blocked_paths_never_reach_the_bus() {
        let mut h = harness().await;
        std::fs::write(h.dir.path().join("a.md"), "x").unwrap();
        h.notify(
            EventKind::Create(CreateKind::File),
            &[
                ".index/seg",
                "memory/vectors.db",
                ".a.md.0badf00d.residuum-tmp",
                "a.md",
            ],
        )
        .await;
        assert_eq!(
            next_event(&mut h.sub).await,
            WorkspaceEvent::Changed(vec![change("a.md", WorkspaceChangeKind::Created)].into())
        );
        h.task.abort();
    }

    #[tokio::test]
    async fn a_prefixed_feed_publishes_namespaced_paths_and_hides_blocked_ones() {
        let mut h = harness_with_prefix(Some("team")).await;
        std::fs::create_dir_all(h.dir.path().join("workbench")).unwrap();
        std::fs::write(h.dir.path().join("workbench").join("tool.html"), "x").unwrap();
        std::fs::write(h.dir.path().join("a.md"), "x").unwrap();
        h.notify(
            EventKind::Create(CreateKind::File),
            &[
                "vectors.db",
                "vectors.db-wal",
                ".index/seg",
                ".a.md.0badf00d.residuum-tmp",
                "workbench/tool.html",
                "a.md",
            ],
        )
        .await;
        assert_eq!(
            next_event(&mut h.sub).await,
            WorkspaceEvent::Changed(
                vec![
                    change("team/a.md", WorkspaceChangeKind::Created),
                    change("team/workbench/tool.html", WorkspaceChangeKind::Created),
                ]
                .into()
            )
        );
        h.task.abort();
    }

    #[tokio::test]
    async fn dropped_notifications_become_a_resync() {
        let mut h = harness().await;
        h.overflowed.store(true, Ordering::Relaxed);
        h.notify(EventKind::Modify(ModifyKind::Any), &["a.md"])
            .await;
        assert_eq!(
            next_event(&mut h.sub).await,
            WorkspaceEvent::Resync(WorkspaceResyncReason::Overflow)
        );

        let rescan = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        h.raw_tx.send(Ok(rescan)).await.unwrap();
        assert_eq!(
            next_event(&mut h.sub).await,
            WorkspaceEvent::Resync(WorkspaceResyncReason::Overflow)
        );
        h.task.abort();
    }

    #[tokio::test]
    async fn the_watch_limit_switches_a_native_watcher_to_polling() {
        let h = harness().await;
        h.raw_tx
            .send(Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch)))
            .await
            .unwrap();
        let exit = crate::testing::wait::guarded("the feed to exit for a polling restart", h.task)
            .await
            .unwrap();
        assert_eq!(exit, LoopExit::Restart(Mode::Polling));
    }

    #[test]
    fn the_sink_drops_events_that_only_touch_hidden_paths() {
        let root = std::path::PathBuf::from("/workspace");
        let (tx, _rx) = mpsc::channel(4);
        let sink = RawSink {
            tx,
            overflowed: Arc::new(AtomicBool::new(false)),
            roots: WorkspaceRoots::new(&root, None),
        };
        let event = |paths: &[&str]| {
            paths.iter().fold(
                notify::Event::new(EventKind::Modify(ModifyKind::Any)),
                |e, p| e.add_path(root.join(p)),
            )
        };

        assert!(
            sink.only_hidden_paths(&event(&["memory/.index/seg.bin", "memory/vectors.db-wal"]))
        );
        assert!(!sink.only_hidden_paths(&event(&["memory/.index/seg.bin", "wiki/a.md"])));
        assert!(!sink.only_hidden_paths(&event(&["wiki/a.md"])));
        assert!(!sink.only_hidden_paths(&event(&[])));
        let rescan = event(&["memory/.index/seg.bin"]).set_flag(notify::event::Flag::Rescan);
        assert!(
            !sink.only_hidden_paths(&rescan),
            "a rescan must always reach the loop"
        );
    }

    #[test]
    fn repeated_warnings_are_throttled_and_counted() {
        let t0 = Instant::now();
        let mut warning = ThrottledWarning::default();
        assert_eq!(warning.should_warn(t0), Some(0));
        assert_eq!(warning.should_warn(t0 + Duration::from_secs(1)), None);
        assert_eq!(warning.should_warn(t0 + Duration::from_secs(2)), None);
        assert_eq!(warning.should_warn(t0 + WARN_INTERVAL), Some(2));
    }

    /// Real OS notifications end to end: whichever watcher this platform
    /// starts, a write, a rename, and hidden files come through as the
    /// design describes.
    #[tokio::test]
    async fn os_notifications_flow_through_the_feed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // Created before the watcher starts, so its own (visible) directory
        // creation isn't part of the change list this test asserts on.
        std::fs::create_dir(root.join("memory")).unwrap();
        let bus = crate::bus::spawn_broker();
        let mut sub = bus.subscribe(topics::Workspace).await.unwrap();
        let (health_tx, mut health_rx) = watch::channel(WatchHealth::Starting);
        let task = spawn_change_feed(root.clone(), None, bus.publisher(), health_tx);
        crate::testing::wait::watch_until(
            "the change feed's watcher to start",
            &mut health_rx,
            |h| *h != WatchHealth::Starting,
        )
        .await;
        crate::workspace::watch::assert_native_watch(*health_rx.borrow(), "the change feed");

        std::fs::create_dir(root.join(".index")).unwrap();
        std::fs::write(root.join(".index").join("seg"), "x").unwrap();
        std::fs::write(root.join("memory").join("vectors.db"), "x").unwrap();
        crate::util::fs::atomic_write(&root.join("a.md"), "hello")
            .await
            .unwrap();
        let first = collect_until(&mut sub, |c| c.path == "a.md").await;
        assert!(
            first.iter().all(|c| c.path == "a.md"),
            "hidden paths leaked into the feed: {first:?}"
        );

        std::fs::rename(root.join("a.md"), root.join("b.md")).unwrap();
        let seen = collect_until(&mut sub, |c| {
            c.path == "b.md" && c.kind == WorkspaceChangeKind::Created
        })
        .await;
        assert!(
            seen.contains(&change("a.md", WorkspaceChangeKind::Removed)),
            "the rename's old path wasn't reported removed: {seen:?}"
        );
        task.abort();
    }

    /// A feed over a directory mounted under a prefix reports OS changes with
    /// the prefix, and a watcher on that prefix receives them.
    #[tokio::test]
    async fn os_notifications_in_a_prefixed_root_carry_the_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir(root.join("workbench")).unwrap();
        let bus = crate::bus::spawn_broker();
        let mut sub = bus.subscribe(topics::Workspace).await.unwrap();
        let (health_tx, mut health_rx) = watch::channel(WatchHealth::Starting);
        let task = spawn_change_feed(root.clone(), Some("team"), bus.publisher(), health_tx);
        crate::testing::wait::watch_until(
            "the change feed's watcher to start",
            &mut health_rx,
            |h| *h != WatchHealth::Starting,
        )
        .await;
        crate::workspace::watch::assert_native_watch(*health_rx.borrow(), "the change feed");

        crate::util::fs::atomic_write(&root.join("workbench").join("tool.html"), "hi")
            .await
            .unwrap();
        let seen = collect_until(&mut sub, |c| c.path == "team/workbench/tool.html").await;
        assert!(
            seen.iter().all(|c| c.path.starts_with("team/")),
            "every path from the team feed carries the prefix: {seen:?}"
        );

        let team_watch =
            crate::workspace::watch::WatchSet::parse(vec!["team/workbench".into()]).unwrap();
        let agent_watch =
            crate::workspace::watch::WatchSet::parse(vec!["workbench".into()]).unwrap();
        assert!(seen.iter().any(|c| team_watch.matches(&c.path)));
        assert!(seen.iter().all(|c| !agent_watch.matches(&c.path)));
        task.abort();
    }

    /// The polling fallback feeds the same loop and reports the same changes.
    #[tokio::test]
    async fn the_polling_fallback_reports_changes() {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let mut sub = bus.subscribe(topics::Workspace).await.unwrap();
        let (raw_tx, raw_rx) = mpsc::channel(RAW_EVENT_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let sink = RawSink {
            tx: raw_tx,
            overflowed: Arc::clone(&overflowed),
            roots: WorkspaceRoots::new(dir.path(), None),
        };
        let backend = start_backend(dir.path(), Mode::Polling, &sink)
            .await
            .unwrap();
        assert_eq!(backend.health(), WatchHealth::Polling);
        let mut feed = FeedLoop::new(dir.path(), None, raw_rx, overflowed, bus.publisher());
        let task = crate::util::spawn_in_span(async move { feed.run(Mode::Polling).await });

        std::fs::write(dir.path().join("polled.md"), "x").unwrap();
        let seen = collect_until(&mut sub, |c| c.path == "polled.md").await;
        assert!(seen.contains(&change("polled.md", WorkspaceChangeKind::Created)));
        task.abort();
        drop(backend);
    }

    /// Collect published changes until one satisfies `done`, failing on a
    /// resync or timeout.
    async fn collect_until(
        sub: &mut Subscriber<WorkspaceEvent>,
        done: impl Fn(&WorkspaceChange) -> bool,
    ) -> Vec<WorkspaceChange> {
        let mut seen = Vec::new();
        loop {
            match next_event(sub).await {
                WorkspaceEvent::Changed(changes) => {
                    seen.extend(changes.iter().cloned());
                    if changes.iter().any(&done) {
                        return seen;
                    }
                }
                other @ (WorkspaceEvent::Resync(_) | WorkspaceEvent::Unavailable) => {
                    panic!("unexpected workspace event: {other:?}")
                }
            }
        }
    }
}
