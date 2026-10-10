//! The devices file: every registered subscription and its preferences.
//!
//! The file holds push endpoints and authentication secrets, so it is written
//! readable only by its owner. Every change reads the file, applies the
//! change and writes it back under one lock, so a delivery result recorded
//! while a device is being registered never loses either.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::PushError;
use super::types::{PushDevice, WebPushSubscription};

/// A device with the subscription notifications are sent to. The API only
/// ever shows the [`PushDevice`] half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredDevice {
    #[serde(flatten)]
    pub(super) device: PushDevice,
    pub(super) subscription: WebPushSubscription,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DevicesFile {
    #[serde(default)]
    devices: Vec<StoredDevice>,
}

/// The devices file and the lock that serializes changes to it.
pub(super) struct DeviceStore {
    path: PathBuf,
    write_lock: tokio::sync::Mutex<()>,
    /// Bumped after every change is saved, so a waiting retry can check
    /// whether its device is still there.
    changes: tokio::sync::watch::Sender<u64>,
}

impl DeviceStore {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: tokio::sync::Mutex::new(()),
            changes: tokio::sync::watch::channel(0).0,
        }
    }

    /// Changes as they are saved: the value moves on after each one.
    pub(super) fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Every device, oldest first. A missing file holds no devices.
    ///
    /// A file that exists but can't be read is an error and is left as it is,
    /// so a fault never turns into the loss of every registration.
    ///
    /// # Errors
    /// Returns [`PushError::Failed`] if the file can't be read or parsed.
    pub(super) async fn read(&self) -> Result<Vec<StoredDevice>, PushError> {
        read_file(&self.path).await
    }

    /// Apply `change` to the devices and save them, when it returns `Some`.
    /// Returning `None` means it changed nothing, and nothing is written.
    ///
    /// # Errors
    /// Returns [`PushError::Failed`] if the file can't be read, parsed or
    /// written.
    pub(super) async fn update<T>(
        &self,
        change: impl FnOnce(&mut Vec<StoredDevice>) -> Option<T>,
    ) -> Result<Option<T>, PushError> {
        let _guard = self.write_lock.lock().await;
        let mut devices = read_file(&self.path).await?;
        let Some(result) = change(&mut devices) else {
            return Ok(None);
        };
        let text = serde_json::to_string_pretty(&DevicesFile { devices }).map_err(|e| {
            tracing::error!(error = %e, "failed to serialize the push devices");
            PushError::Failed(format!(
                "Residuum couldn't save the notification devices: {e}."
            ))
        })?;
        crate::util::fs::atomic_write_owner_only(&self.path, text)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, path = %self.path.display(), "failed to write the push devices file");
                PushError::Failed(format!(
                    "Residuum couldn't save the notification devices to {}: {e:#}.",
                    self.path.display()
                ))
            })?;
        self.changes.send_modify(|version| *version += 1);
        Ok(Some(result))
    }
}

async fn read_file(path: &Path) -> Result<Vec<StoredDevice>, PushError> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => serde_json::from_str::<DevicesFile>(&text)
            .map(|file| file.devices)
            .map_err(|e| {
                tracing::error!(error = %e, path = %path.display(), "the push devices file can't be parsed");
                PushError::Failed(format!(
                    "The saved list of notification devices at {} can't be read ({e}). Residuum \
                     left it alone. To start over, move that file away and turn notifications \
                     on again on each device.",
                    path.display()
                ))
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => {
            tracing::error!(error = %e, path = %path.display(), "failed to read the push devices file");
            Err(PushError::Failed(format!(
                "Residuum couldn't read the saved notification devices at {}: {e}.",
                path.display()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::hub::push::types::{PushPreferences, WebPushSubscriptionKeys};

    fn device(id: &str, endpoint: &str) -> StoredDevice {
        StoredDevice {
            device: PushDevice {
                id: id.to_string(),
                label: format!("device {id}"),
                created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
                last_success_at: None,
                last_failure: None,
                preferences: PushPreferences::default(),
            },
            subscription: WebPushSubscription {
                endpoint: endpoint.to_string(),
                keys: WebPushSubscriptionKeys {
                    p256dh: "p".to_string(),
                    auth: "a".to_string(),
                },
            },
        }
    }

    #[tokio::test]
    async fn a_missing_file_holds_no_devices() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeviceStore::new(dir.path().join("push-devices.json"));
        assert!(store.read().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn changes_are_saved_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeviceStore::new(dir.path().join("push-devices.json"));

        let saved = store
            .update(|devices| {
                devices.push(device("a", "https://push.example/a"));
                Some("done")
            })
            .await
            .unwrap();
        assert_eq!(saved, Some("done"));

        assert_eq!(
            store.read().await.unwrap(),
            vec![device("a", "https://push.example/a")]
        );
    }

    #[tokio::test]
    async fn a_change_that_returns_none_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("push-devices.json");
        let store = DeviceStore::new(path.clone());

        let result = store
            .update(|devices| {
                devices.push(device("a", "https://push.example/a"));
                None::<()>
            })
            .await
            .unwrap();
        assert_eq!(result, None);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_parsed_is_reported_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("push-devices.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = DeviceStore::new(path.clone());

        let read_err = store.read().await.unwrap_err();
        assert!(
            read_err.to_string().contains("push-devices.json"),
            "{read_err}"
        );
        let update_err = store
            .update(|devices| {
                devices.push(device("a", "https://push.example/a"));
                Some(())
            })
            .await
            .unwrap_err();
        assert!(matches!(update_err, PushError::Failed(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("push-devices.json");
        let store = DeviceStore::new(path.clone());
        store
            .update(|devices| {
                devices.push(device("a", "https://push.example/a"));
                Some(())
            })
            .await
            .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn concurrent_changes_all_land() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(DeviceStore::new(dir.path().join("push-devices.json")));

        let mut tasks = Vec::new();
        for i in 0..12 {
            let store = std::sync::Arc::clone(&store);
            tasks.push(crate::util::spawn_in_span(async move {
                store
                    .update(|devices| {
                        devices.push(device(&i.to_string(), &format!("https://push.example/{i}")));
                        Some(())
                    })
                    .await
                    .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(store.read().await.unwrap().len(), 12);
    }
}
