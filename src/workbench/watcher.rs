//! Polls the workbench directory and publishes a [`WorkbenchEvent`] whenever a
//! tool page is added, changed, or removed, so open tool views reload live.
//!
//! Only tool pages are watched. A tool saving its own data file
//! (`<name>.state.json`) must not reload the tool, or it would lose its
//! in-memory state on every save.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;

use crate::bus::{Publisher, WorkbenchEvent, topics};

const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// What identifies one version of a tool page.
type PageStamp = (Option<SystemTime>, u64);

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
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e),
    };
    let mut pages = HashMap::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str().and_then(super::tool_name_of) else {
            continue;
        };
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let metadata = entry.metadata().await?;
        pages.insert(name.to_string(), (metadata.modified().ok(), metadata.len()));
    }
    Ok(pages)
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
    async fn scan_watches_only_tool_pages() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chart.html"), "x").unwrap();
        std::fs::write(dir.path().join("chart.state.json"), "{}").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let pages = scan(dir.path()).await.unwrap();
        assert_eq!(pages.keys().collect::<Vec<_>>(), ["chart"]);
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
