//! Derives artifact reloads from the workspace change feed: whenever a batch
//! touches the workbench folder (or the feed asks watchers to resync), the
//! workbench is rescanned and a [`WorkbenchEvent`] is published for each
//! artifact that was added, changed, or removed, so open artifact views reload
//! live.
//!
//! An artifact is its page, or every file inside its folder. An artifact's
//! saved data sits beside it (`<name>.state.json`) and is not part of it, so
//! an artifact saving its state never reloads itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::task::JoinHandle;

use crate::bus::{BusError, BusHandle, Publisher, WorkbenchEvent, WorkspaceEvent, topics};
use crate::workspace::layout::WORKBENCH_DIR;

/// What identifies one version of an artifact: newest file time, total size, and
/// file count (so adding or removing a file registers even when times don't
/// move).
type PageStamp = (Option<SystemTime>, u64, usize);

/// Subscribe to the workspace change feed, take the workbench's current
/// state, and spawn the task that publishes artifact reloads. The task runs
/// until aborted or the bus shuts down.
///
/// # Errors
/// Returns an error if the change feed subscription fails.
pub(crate) async fn spawn_workbench_watcher(
    dir: PathBuf,
    bus: &BusHandle,
    publisher: Publisher,
) -> Result<JoinHandle<()>, BusError> {
    let mut feed = bus.subscribe(topics::Workspace).await?;
    let mut scanner = WorkbenchScanner::new(dir);
    let mut known = scanner.scan().await.unwrap_or_default();

    Ok(tokio::spawn(async move {
        loop {
            let event = match feed.recv().await {
                Ok(Some(event)) => event,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to read the workspace change feed; open artifacts won't reload on their own");
                    return;
                }
            };
            if !affects_workbench(&event) {
                continue;
            }
            let Some(current) = scanner.scan().await else {
                continue;
            };
            for change in diff(&known, &current) {
                if let Err(e) = publisher.publish(topics::Workbench, change).await {
                    tracing::warn!(error = %e, "failed to publish workbench change; stopping the artifact reload watcher");
                    return;
                }
            }
            known = current;
        }
    }))
}

/// Whether a change-feed event may have changed an artifact.
fn affects_workbench(event: &WorkspaceEvent) -> bool {
    match event {
        WorkspaceEvent::Changed(changes) => changes.iter().any(|c| {
            c.path
                .strip_prefix(WORKBENCH_DIR)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        }),
        // Changes were missed, so any artifact may have changed.
        WorkspaceEvent::Resync(_) => true,
        WorkspaceEvent::Unavailable => false,
    }
}

/// Scans the workbench, logging a failure once until a scan succeeds again.
struct WorkbenchScanner {
    dir: PathBuf,
    failing: bool,
}

impl WorkbenchScanner {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            failing: false,
        }
    }

    /// The current artifacts, or `None` when the workbench can't be read.
    async fn scan(&mut self) -> Option<HashMap<String, PageStamp>> {
        match scan(&self.dir).await {
            Ok(pages) => {
                if std::mem::take(&mut self.failing) {
                    tracing::info!(dir = %self.dir.display(), "workbench directory readable again; live reload resumed");
                }
                Some(pages)
            }
            Err(e) => {
                if !self.failing {
                    tracing::warn!(dir = %self.dir.display(), error = %e, "failed to scan the workbench directory; live reload is paused until it can be read");
                    self.failing = true;
                }
                None
            }
        }
    }
}

async fn scan(dir: &Path) -> std::io::Result<HashMap<String, PageStamp>> {
    Ok(super::discover_artifacts(dir)
        .await?
        .into_iter()
        .map(|artifact| {
            (
                artifact.name,
                (artifact.modified, artifact.size, artifact.files),
            )
        })
        .collect())
}

fn diff(
    before: &HashMap<String, PageStamp>,
    after: &HashMap<String, PageStamp>,
) -> Vec<WorkbenchEvent> {
    let mut events: Vec<WorkbenchEvent> = after
        .iter()
        .filter(|(name, stamp)| before.get(*name) != Some(stamp))
        .map(|(name, _)| WorkbenchEvent::Updated { name: name.clone() })
        .collect();
    events.extend(
        before
            .keys()
            .filter(|name| !after.contains_key(*name))
            .map(|name| WorkbenchEvent::Removed { name: name.clone() }),
    );
    events
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::bus::Subscriber;
    use crate::workspace::watch::{WorkspaceChange, WorkspaceChangeKind, WorkspaceResyncReason};

    fn stamp(secs: u64, len: u64) -> PageStamp {
        (
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
            len,
            1,
        )
    }

    fn changed(paths: &[&str]) -> WorkspaceEvent {
        WorkspaceEvent::Changed(
            paths
                .iter()
                .map(|p| WorkspaceChange {
                    path: (*p).to_string(),
                    kind: WorkspaceChangeKind::Modified,
                })
                .collect(),
        )
    }

    #[test]
    fn diff_reports_added_changed_and_removed_pages() {
        let before = HashMap::from([
            ("same".to_string(), stamp(1, 10)),
            ("edited".to_string(), stamp(1, 10)),
            ("gone".to_string(), stamp(1, 10)),
        ]);
        let after = HashMap::from([
            ("same".to_string(), stamp(1, 10)),
            ("edited".to_string(), stamp(1, 11)),
            ("new".to_string(), stamp(2, 5)),
        ]);
        let mut events = diff(&before, &after);
        events.sort_by_key(|e| format!("{e:?}"));
        assert_eq!(
            events,
            [
                WorkbenchEvent::Removed {
                    name: "gone".into()
                },
                WorkbenchEvent::Updated {
                    name: "edited".into()
                },
                WorkbenchEvent::Updated { name: "new".into() },
            ]
        );
    }

    #[tokio::test]
    async fn scan_watches_only_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chart.html"), "x").unwrap();
        std::fs::write(dir.path().join("chart.state.json"), "{}").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let pages = scan(dir.path()).await.unwrap();
        assert_eq!(pages.keys().collect::<Vec<_>>(), ["chart"]);
    }

    #[test]
    fn only_workbench_changes_and_resyncs_trigger_a_rescan() {
        assert!(affects_workbench(&changed(&["workbench/chart.html"])));
        assert!(affects_workbench(&changed(&["wiki/a.md", "workbench"])));
        assert!(!affects_workbench(&changed(&["workbenches/x.html"])));
        assert!(!affects_workbench(&changed(&["wiki/a.md"])));
        assert!(affects_workbench(&WorkspaceEvent::Resync(
            WorkspaceResyncReason::Overflow
        )));
        assert!(!affects_workbench(&WorkspaceEvent::Unavailable));
    }

    struct Harness {
        workspace: tempfile::TempDir,
        bus: BusHandle,
        reloads: Subscriber<WorkbenchEvent>,
        task: JoinHandle<()>,
    }

    async fn harness() -> Harness {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("workbench")).unwrap();
        let bus = crate::bus::spawn_broker();
        let reloads = bus.subscribe(topics::Workbench).await.unwrap();
        let task =
            spawn_workbench_watcher(workspace.path().join("workbench"), &bus, bus.publisher())
                .await
                .unwrap();
        Harness {
            workspace,
            bus,
            reloads,
            task,
        }
    }

    impl Harness {
        fn write(&self, relative: &str, content: &str) {
            let path = self.workspace.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        async fn feed(&self, event: WorkspaceEvent) {
            self.bus
                .publisher()
                .publish(topics::Workspace, event)
                .await
                .unwrap();
        }

        async fn next_reload(&mut self) -> WorkbenchEvent {
            tokio::time::timeout(Duration::from_secs(10), self.reloads.recv())
                .await
                .expect("timed out waiting for an artifact reload")
                .unwrap()
                .unwrap()
        }
    }

    #[tokio::test]
    async fn page_changes_reload_the_artifact_and_data_files_do_not() {
        let mut h = harness().await;
        h.write("workbench/chart.state.json", "{}");
        h.feed(changed(&["workbench/chart.state.json"])).await;
        h.write("workbench/chart.html", "<p>v1</p>");
        h.feed(changed(&["workbench/chart.html"])).await;
        // The data file's batch came first; had it reloaded anything, that
        // reload would arrive before this one.
        assert_eq!(
            h.next_reload().await,
            WorkbenchEvent::Updated {
                name: "chart".into()
            }
        );

        std::fs::remove_file(h.workspace.path().join("workbench/chart.html")).unwrap();
        h.feed(changed(&["workbench/chart.html"])).await;
        assert_eq!(
            h.next_reload().await,
            WorkbenchEvent::Removed {
                name: "chart".into()
            }
        );
        h.task.abort();
    }

    #[tokio::test]
    async fn any_file_in_a_folder_artifact_reloads_it() {
        let mut h = harness().await;
        h.write("workbench/graph/index.html", "<p>graph</p>");
        h.feed(changed(&["workbench/graph/index.html"])).await;
        assert_eq!(
            h.next_reload().await,
            WorkbenchEvent::Updated {
                name: "graph".into()
            }
        );

        h.write("workbench/graph/data/points.json", "[]");
        h.feed(changed(&["workbench/graph/data/points.json"])).await;
        assert_eq!(
            h.next_reload().await,
            WorkbenchEvent::Updated {
                name: "graph".into()
            }
        );

        std::fs::remove_dir_all(h.workspace.path().join("workbench/graph")).unwrap();
        h.feed(changed(&["workbench/graph"])).await;
        assert_eq!(
            h.next_reload().await,
            WorkbenchEvent::Removed {
                name: "graph".into()
            }
        );
        h.task.abort();
    }

    #[tokio::test]
    async fn a_resync_catches_up_on_missed_artifact_changes() {
        let mut h = harness().await;
        h.write("workbench/missed.html", "x");
        h.feed(WorkspaceEvent::Resync(WorkspaceResyncReason::Overflow))
            .await;
        assert_eq!(
            h.next_reload().await,
            WorkbenchEvent::Updated {
                name: "missed".into()
            }
        );
        h.task.abort();
    }
}
