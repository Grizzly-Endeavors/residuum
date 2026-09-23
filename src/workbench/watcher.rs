//! Polls the workbench directory and publishes a [`WorkbenchEvent`] whenever an
//! artifact is added, changed, or removed, so open artifact views reload live.
//!
//! Only artifacts are watched: a page, or any file inside a folder artifact. An artifact's
//! saved data sits beside it (`<name>.state.json`) and is not watched, so an
//! artifact saving its state never reloads itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;

use crate::bus::{Publisher, WorkbenchEvent, topics};

const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// What identifies one version of an artifact: newest file time, total size, and
/// file count (so adding or removing a file registers even when times don't
/// move).
type PageStamp = (Option<SystemTime>, u64, usize);

/// Spawn the workbench watcher. Runs until aborted.
pub(crate) fn spawn_workbench_watcher(dir: PathBuf, publisher: Publisher) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_scan_failed = false;
        let mut known = match scan(&dir).await {
            Ok(pages) => pages,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "failed to scan the workbench directory; live reload is paused until it can be read");
                last_scan_failed = true;
                HashMap::new()
            }
        };
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.tick().await;

        loop {
            interval.tick().await;
            let current = match scan(&dir).await {
                Ok(pages) => {
                    if last_scan_failed {
                        tracing::info!(dir = %dir.display(), "workbench directory readable again; live reload resumed");
                        last_scan_failed = false;
                    }
                    pages
                }
                Err(e) => {
                    if !last_scan_failed {
                        tracing::warn!(dir = %dir.display(), error = %e, "failed to scan the workbench directory; live reload is paused until it can be read");
                        last_scan_failed = true;
                    }
                    continue;
                }
            };

            for event in diff(&known, &current) {
                if let Err(e) = publisher.publish(topics::Workbench, event).await {
                    tracing::warn!(error = %e, "failed to publish workbench change; stopping the workbench watcher");
                    return;
                }
            }
            known = current;
        }
    })
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
    use super::*;

    fn stamp(secs: u64, len: u64) -> PageStamp {
        (
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
            len,
            1,
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

    #[tokio::test]
    async fn a_new_file_in_a_folder_artifact_changes_its_stamp() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("graph")).unwrap();
        std::fs::write(dir.path().join("graph/index.html"), "x").unwrap();
        let before = scan(dir.path()).await.unwrap();
        std::fs::write(dir.path().join("graph/data.json"), "[]").unwrap();
        let after = scan(dir.path()).await.unwrap();
        assert_eq!(
            diff(&before, &after),
            [WorkbenchEvent::Updated {
                name: "graph".into()
            }]
        );
    }

    #[tokio::test]
    async fn watcher_publishes_page_changes() {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let mut sub = bus.subscribe(topics::Workbench).await.unwrap();
        let handle = spawn_workbench_watcher(dir.path().to_path_buf(), bus.publisher());

        // Let the watcher take its initial snapshot before the page appears.
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(dir.path().join("chart.html"), "x").unwrap();
        let added = tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            added,
            Some(WorkbenchEvent::Updated {
                name: "chart".into()
            })
        );

        std::fs::remove_file(dir.path().join("chart.html")).unwrap();
        let removed = tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            removed,
            Some(WorkbenchEvent::Removed {
                name: "chart".into()
            })
        );
        handle.abort();
    }
}
