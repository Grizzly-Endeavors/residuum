//! The Web Push service: the hub's devices, its signing key, and delivery.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::{OnceCell, watch};

use super::deliver::{Attempt, Failure, send_once};
use super::devices::{DeviceStore, StoredDevice};
use super::encrypt::Recipient;
use super::error::PushError;
use super::keys::VapidKey;
use super::types::{
    PatchPushDeviceRequest, PushDevice, PushEvent, PushFailure, PushMessage, PushPayload,
    PushPreferences, PushTestResult, PutPushDeviceRequest, WebPushSubscription,
};
use crate::config::HubPaths;

/// The VAPID `sub` claim when the hub config sets no `[push] contact`.
pub const DEFAULT_CONTACT: &str = "https://github.com/Grizzly-Endeavors/residuum";

/// How long a delivery that failed in a way that may pass waits before its
/// one retry.
const RETRY_DELAY: Duration = Duration::from_secs(30);

/// The name a device gets when its registration gives none.
const UNNAMED_DEVICE: &str = "Unnamed device";

/// Web Push for the whole hub: registered devices, the signing key, and
/// sending.
///
/// Delivery never blocks or fails the code that triggers it: [`notify`]
/// returns at once and logs what goes wrong. A device whose delivery fails
/// shows the failure in its `last_failure`; a device the push service says is
/// gone is removed.
///
/// [`notify`]: PushService::notify
pub struct PushService {
    key_path: std::path::PathBuf,
    store: DeviceStore,
    key: OnceCell<VapidKey>,
    subject: watch::Sender<String>,
    retry_delay: Duration,
}

impl PushService {
    /// The service for the hub directory `hub_dir`, whose key and devices
    /// files it keeps there. `contact` is the hub config's `[push] contact`.
    #[must_use]
    pub fn new(hub_dir: &Path, contact: Option<&str>) -> Arc<Self> {
        Arc::new(Self::build(hub_dir, contact, RETRY_DELAY))
    }

    fn build(hub_dir: &Path, contact: Option<&str>, retry_delay: Duration) -> Self {
        let paths = HubPaths::new(hub_dir);
        Self {
            key_path: paths.push_vapid_key(),
            store: DeviceStore::new(paths.push_devices_json()),
            key: OnceCell::new(),
            subject: watch::channel(subject_for(contact)).0,
            retry_delay,
        }
    }

    /// Use `contact` (the hub config's `[push] contact`) as the VAPID `sub`
    /// claim of every push sent from now on.
    pub fn set_contact(&self, contact: Option<&str>) {
        self.subject.send_replace(subject_for(contact));
    }

    /// The VAPID `sub` claim pushes are sent with now.
    #[must_use]
    pub fn subject(&self) -> String {
        self.subject.borrow().clone()
    }

    async fn vapid_key(&self) -> Result<&VapidKey, PushError> {
        self.key
            .get_or_try_init(|| VapidKey::load_or_create(&self.key_path))
            .await
    }

    /// The hub's VAPID public key as base64url, which a browser subscribes
    /// with. The key is created the first time it is needed.
    ///
    /// # Errors
    /// Returns [`PushError::Failed`] if the key can't be read or created.
    pub async fn public_key(&self) -> Result<String, PushError> {
        Ok(self.vapid_key().await?.public_key_base64url())
    }

    /// Every registered device, oldest first.
    ///
    /// # Errors
    /// Returns [`PushError::Failed`] if the devices file can't be read.
    pub async fn devices(&self) -> Result<Vec<PushDevice>, PushError> {
        Ok(self
            .store
            .read()
            .await?
            .into_iter()
            .map(|stored| stored.device)
            .collect())
    }

    /// Register a device, or update the one already registered for the same
    /// subscription endpoint, which keeps its id and delivery history.
    ///
    /// # Errors
    /// Returns [`PushError::BadRequest`] for a blank label and
    /// [`PushError::Failed`] if the devices file can't be read or written.
    pub async fn upsert_device(
        &self,
        request: PutPushDeviceRequest,
    ) -> Result<PushDevice, PushError> {
        let PutPushDeviceRequest {
            subscription,
            label,
            preferences,
        } = request;
        let label = label.map(|raw| clean_label(&raw)).transpose()?;
        let updated = self
            .store
            .update(|devices| {
                let existing = devices
                    .iter()
                    .position(|d| d.subscription.endpoint == subscription.endpoint);
                let device = if let Some(index) = existing {
                    devices.get_mut(index)?
                } else {
                    devices.push(new_device(subscription.clone()));
                    devices.last_mut()?
                };
                device.subscription = subscription;
                if let Some(label) = label {
                    device.device.label = label;
                }
                if let Some(patch) = preferences {
                    device.device.preferences = device.device.preferences.patched(patch);
                }
                Some(device.device.clone())
            })
            .await?;
        updated.ok_or_else(|| PushError::Failed("the device could not be saved".to_string()))
    }

    /// Change a device's label and/or preferences.
    ///
    /// # Errors
    /// Returns [`PushError::BadRequest`] for a request that changes nothing or
    /// a blank label, [`PushError::UnknownDevice`] for an unknown id, and
    /// [`PushError::Failed`] if the devices file can't be read or written.
    pub async fn patch_device(
        &self,
        id: &str,
        request: PatchPushDeviceRequest,
    ) -> Result<PushDevice, PushError> {
        if request.label.is_none() && request.preferences.is_none() {
            return Err(PushError::BadRequest(
                "the request must set a label or preferences".to_string(),
            ));
        }
        let label = request.label.map(|raw| clean_label(&raw)).transpose()?;
        let updated = self
            .store
            .update(|devices| {
                let device = devices.iter_mut().find(|d| d.device.id == id)?;
                if let Some(label) = label {
                    device.device.label = label;
                }
                if let Some(patch) = request.preferences {
                    device.device.preferences = device.device.preferences.patched(patch);
                }
                Some(device.device.clone())
            })
            .await?;
        updated.ok_or_else(|| PushError::UnknownDevice(id.to_string()))
    }

    /// Remove a device. It stops receiving notifications at once; the
    /// browser's own subscription is the browser's to cancel.
    ///
    /// # Errors
    /// Returns [`PushError::UnknownDevice`] for an unknown id and
    /// [`PushError::Failed`] if the devices file can't be read or written.
    pub async fn delete_device(&self, id: &str) -> Result<(), PushError> {
        let removed = self
            .store
            .update(|devices| {
                let before = devices.len();
                devices.retain(|d| d.device.id != id);
                (devices.len() != before).then_some(())
            })
            .await?;
        removed.ok_or_else(|| PushError::UnknownDevice(id.to_string()))
    }

    /// Send the test notification to one device and report how it went. The
    /// device's preferences don't matter, and the delivery is tried once: the
    /// person asking is waiting for the answer.
    ///
    /// `badge` is the user's total unread inbox count, which the
    /// notification sets on the app badge as every notification does.
    ///
    /// # Errors
    /// Returns [`PushError::UnknownDevice`] for an unknown id and
    /// [`PushError::Failed`] if the devices file can't be read or written. A
    /// notification the push service refuses is not an error: it comes back
    /// as a result with `delivered` false.
    pub async fn send_test(&self, id: &str, badge: u32) -> Result<PushTestResult, PushError> {
        let stored = self
            .store
            .read()
            .await?
            .into_iter()
            .find(|d| d.device.id == id)
            .ok_or_else(|| PushError::UnknownDevice(id.to_string()))?;
        let message = PushMessage::new(PushPayload::new(
            PushEvent::Test,
            "",
            "Residuum test notification",
            &format!("Notifications are working on {}.", stored.device.label),
            "/home",
            "test",
            badge,
        ));
        Ok(self.deliver(stored, &message, false).await)
    }

    /// Send `message` in the background to every device whose preferences turn
    /// its event on, except the devices in `skip` (the ones whose user is
    /// looking at the app). Returns at once: nothing here blocks or fails the
    /// caller, and what goes wrong is logged and recorded on the device.
    pub fn notify(self: &Arc<Self>, message: PushMessage, skip: HashSet<String>) {
        let service = Arc::clone(self);
        crate::util::spawn_in_span(async move {
            let devices = match service.store.read().await {
                Ok(devices) => devices,
                Err(e) => {
                    tracing::warn!(error = %e, event = ?message.payload.event, "no push was sent, because the notification devices couldn't be read");
                    return;
                }
            };
            let message = Arc::new(message);
            for stored in devices.into_iter().filter(|d| {
                d.device.preferences.wants(message.payload.event) && !skip.contains(&d.device.id)
            }) {
                let service = Arc::clone(&service);
                let message = Arc::clone(&message);
                crate::util::spawn_in_span(async move {
                    service.deliver(stored, &message, true).await;
                });
            }
        });
    }

    /// Deliver `message` to one device and record how it went. With `retry`,
    /// a failure that may pass is tried once more after the retry delay.
    async fn deliver(
        &self,
        stored: StoredDevice,
        message: &PushMessage,
        retry: bool,
    ) -> PushTestResult {
        let mut attempt = self.attempt(&stored.subscription, message).await;
        let mut retried = false;
        if retry && let Attempt::Retryable(first) = &attempt {
            tracing::warn!(
                device = %stored.device.label,
                status = ?first.status,
                error = %first.message,
                response = ?first.response,
                retry_in_secs = self.retry_delay.as_secs(),
                "push delivery failed, retrying once"
            );
            tokio::time::sleep(self.retry_delay).await;
            // The device may have been removed or re-registered meanwhile.
            match self.current_subscription(&stored.device.id).await {
                Some(current) if current == stored.subscription => {
                    attempt = self.attempt(&stored.subscription, message).await;
                    retried = true;
                }
                _ => {
                    tracing::debug!(device = %stored.device.label, "the device changed while a push waited to be retried, so it was dropped");
                    return PushTestResult {
                        delivered: false,
                        error: Some(
                            "The device changed before the notification could be sent again."
                                .to_string(),
                        ),
                    };
                }
            }
        }
        self.conclude(&stored.device, attempt, retried).await
    }

    async fn current_subscription(&self, id: &str) -> Option<WebPushSubscription> {
        match self.store.read().await {
            Ok(devices) => devices
                .into_iter()
                .find(|d| d.device.id == id)
                .map(|d| d.subscription),
            Err(e) => {
                tracing::warn!(error = %e, "couldn't reread the notification devices before a push retry");
                None
            }
        }
    }

    async fn attempt(&self, subscription: &WebPushSubscription, message: &PushMessage) -> Attempt {
        let key = match self.vapid_key().await {
            Ok(key) => key,
            Err(e) => {
                return Attempt::Rejected(Failure {
                    status: None,
                    message: e.to_string(),
                    response: None,
                });
            }
        };
        send_once(
            subscription,
            key,
            &self.subject(),
            message,
            Utc::now().timestamp(),
        )
        .await
    }

    /// Record what `attempt` came to on the device, and say how it went.
    async fn conclude(
        &self,
        device: &PushDevice,
        attempt: Attempt,
        retried: bool,
    ) -> PushTestResult {
        match attempt {
            Attempt::Delivered => {
                if retried {
                    tracing::info!(device = %device.label, "push delivery succeeded on the retry");
                }
                self.record(&device.id, |d| d.last_success_at = Some(Utc::now()))
                    .await;
                PushTestResult {
                    delivered: true,
                    error: None,
                }
            }
            Attempt::Gone => {
                tracing::warn!(device = %device.label, "the push service no longer knows this device, so it was removed");
                self.remove_gone(&device.id).await;
                PushTestResult {
                    delivered: false,
                    error: Some(
                        "This device's notifications were turned off or expired, so it was \
                         removed from the list. Turn notifications on again on that device."
                            .to_string(),
                    ),
                }
            }
            Attempt::Retryable(failure) | Attempt::Rejected(failure) => {
                tracing::warn!(
                    device = %device.label,
                    status = ?failure.status,
                    error = %failure.message,
                    response = ?failure.response,
                    retried,
                    "push delivery failed"
                );
                let recorded = PushFailure {
                    at: Utc::now(),
                    status: failure.status,
                    message: failure.message.clone(),
                };
                self.record(&device.id, |d| d.last_failure = Some(recorded))
                    .await;
                PushTestResult {
                    delivered: false,
                    error: Some(failure.message),
                }
            }
        }
    }

    /// Change a device's delivery history, if the device still exists.
    async fn record(&self, id: &str, change: impl FnOnce(&mut PushDevice)) {
        let result = self
            .store
            .update(|devices| {
                let stored = devices.iter_mut().find(|d| d.device.id == id)?;
                change(&mut stored.device);
                Some(())
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, device_id = id, "couldn't record the result of a push delivery");
        }
    }

    async fn remove_gone(&self, id: &str) {
        if let Err(e) = self.delete_device(id).await
            && !matches!(e, PushError::UnknownDevice(_))
        {
            tracing::warn!(error = %e, device_id = id, "couldn't remove a device the push service no longer knows");
        }
    }
}

/// The VAPID `sub` claim for a configured contact.
fn subject_for(contact: Option<&str>) -> String {
    contact.unwrap_or(DEFAULT_CONTACT).to_string()
}

fn clean_label(raw: &str) -> Result<String, PushError> {
    let label = raw.trim();
    if label.is_empty() {
        return Err(PushError::BadRequest("a device needs a name".to_string()));
    }
    Ok(label.to_string())
}

fn new_device(subscription: WebPushSubscription) -> StoredDevice {
    StoredDevice {
        device: PushDevice {
            id: uuid::Uuid::new_v4().to_string(),
            label: UNNAMED_DEVICE.to_string(),
            created_at: Utc::now(),
            last_success_at: None,
            last_failure: None,
            preferences: PushPreferences::default(),
        },
        subscription,
    }
}

/// Check a subscription the API received: an `https:` endpoint, and keys of
/// the right shape.
///
/// # Errors
/// Returns [`PushError::BadRequest`] saying what is wrong with it.
pub fn validate_subscription(subscription: &WebPushSubscription) -> Result<(), PushError> {
    let endpoint = url::Url::parse(&subscription.endpoint).map_err(|e| {
        PushError::BadRequest(format!(
            "the subscription's endpoint isn't a web address ({e})"
        ))
    })?;
    if endpoint.scheme() != "https" {
        return Err(PushError::BadRequest(
            "the subscription's endpoint must be an https:// address".to_string(),
        ));
    }
    Recipient::parse(&subscription.keys)
        .map(|_| ())
        .map_err(PushError::BadRequest)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use base64::Engine as _;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::hub::push::encrypt::browser::Browser;
    use crate::hub::push::types::PushPreferencesPatch;

    const FAST_RETRY: Duration = Duration::from_millis(40);

    fn service_in(dir: &tempfile::TempDir, retry_delay: Duration) -> Arc<PushService> {
        Arc::new(PushService::build(dir.path(), None, retry_delay))
    }

    fn put(endpoint: &str, browser: &Browser, label: &str) -> PutPushDeviceRequest {
        PutPushDeviceRequest {
            subscription: WebPushSubscription {
                endpoint: endpoint.to_string(),
                keys: browser.keys(),
            },
            label: Some(label.to_string()),
            preferences: None,
        }
    }

    /// Register a device whose push endpoint is `server`.
    async fn register(service: &PushService, server: &MockServer, label: &str) -> PushDevice {
        service
            .upsert_device(put(
                &format!("{}/push/{label}", server.uri()),
                &Browser::new(),
                label,
            ))
            .await
            .unwrap()
    }

    async fn find(service: &PushService, id: &str) -> Option<PushDevice> {
        service
            .devices()
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.id == id)
    }

    /// Wait for the device to reach the state `done` describes, as a
    /// background delivery records its result.
    async fn settle(
        service: &PushService,
        id: &str,
        done: impl Fn(Option<&PushDevice>) -> bool,
    ) -> Option<PushDevice> {
        for _ in 0..250 {
            let device = find(service, id).await;
            if done(device.as_ref()) {
                return device;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the device never reached the expected state");
    }

    fn sample_message(event: PushEvent) -> PushMessage {
        PushMessage::new(PushPayload::new(
            event, "scout", "A title", "A body", "/inbox", "tag", 3,
        ))
    }

    async fn requests_to(server: &MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    async fn accepts(server: &MockServer, status: u16) {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn registering_the_same_endpoint_again_updates_the_device() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let browser = Browser::new();

        let first = service
            .upsert_device(put("https://push.example/a", &browser, "Laptop"))
            .await
            .unwrap();
        let mut again = put("https://push.example/a", &browser, "  Work laptop ");
        again.preferences = Some(PushPreferencesPatch {
            reply_while_away: Some(true),
            ..PushPreferencesPatch::default()
        });
        let second = service.upsert_device(again).await.unwrap();

        assert_eq!(second.id, first.id, "the endpoint identifies the device");
        assert_eq!(second.created_at, first.created_at);
        assert_eq!(second.label, "Work laptop");
        assert!(second.preferences.wants(PushEvent::ReplyWhileAway));
        assert!(
            second.preferences.wants(PushEvent::InboxItem),
            "unnamed preferences stay"
        );
        assert_eq!(service.devices().await.unwrap(), vec![second]);

        let other = service
            .upsert_device(put("https://push.example/b", &Browser::new(), "Phone"))
            .await
            .unwrap();
        assert_ne!(other.id, first.id);
        assert_eq!(service.devices().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_new_device_starts_with_inbox_and_failure_notifications_on() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let mut request = put("https://push.example/a", &Browser::new(), "x");
        request.label = None;

        let device = service.upsert_device(request).await.unwrap();

        assert_eq!(device.label, "Unnamed device");
        assert_eq!(device.preferences, PushPreferences::default());
        assert!(device.last_success_at.is_none() && device.last_failure.is_none());
    }

    #[tokio::test]
    async fn a_device_never_shows_its_endpoint_or_keys() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = service
            .upsert_device(put("https://push.example/secret", &Browser::new(), "x"))
            .await
            .unwrap();

        let json = serde_json::to_value(&device).unwrap();
        let mut fields: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "created_at",
                "id",
                "label",
                "last_failure",
                "last_success_at",
                "preferences"
            ]
        );
    }

    #[tokio::test]
    async fn patching_changes_the_label_and_only_the_named_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = service
            .upsert_device(put("https://push.example/a", &Browser::new(), "Laptop"))
            .await
            .unwrap();

        let patched = service
            .patch_device(
                &device.id,
                PatchPushDeviceRequest {
                    label: Some("Desk".to_string()),
                    preferences: Some(PushPreferencesPatch {
                        inbox_item: Some(false),
                        outbound_unreachable: Some(true),
                        ..PushPreferencesPatch::default()
                    }),
                },
            )
            .await
            .unwrap();

        assert_eq!(patched.label, "Desk");
        assert_eq!(
            serde_json::to_value(&patched.preferences).unwrap(),
            serde_json::json!({
                "inbox_item": false,
                "agent_failed": true,
                "outbound_unreachable": true,
                "reply_while_away": false,
            })
        );
        assert_eq!(find(&service, &device.id).await, Some(patched));
    }

    #[tokio::test]
    async fn a_patch_must_change_something_and_name_a_known_device() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = service
            .upsert_device(put("https://push.example/a", &Browser::new(), "Laptop"))
            .await
            .unwrap();

        let nothing = PatchPushDeviceRequest {
            label: None,
            preferences: None,
        };
        assert!(matches!(
            service.patch_device(&device.id, nothing).await,
            Err(PushError::BadRequest(_))
        ));
        let blank = PatchPushDeviceRequest {
            label: Some("   ".to_string()),
            preferences: None,
        };
        assert!(matches!(
            service.patch_device(&device.id, blank).await,
            Err(PushError::BadRequest(_))
        ));
        let unknown = PatchPushDeviceRequest {
            label: Some("x".to_string()),
            preferences: None,
        };
        assert!(matches!(
            service.patch_device("nope", unknown).await,
            Err(PushError::UnknownDevice(_))
        ));
    }

    #[tokio::test]
    async fn deleting_removes_the_device_once() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = service
            .upsert_device(put("https://push.example/a", &Browser::new(), "Laptop"))
            .await
            .unwrap();

        service.delete_device(&device.id).await.unwrap();
        assert!(service.devices().await.unwrap().is_empty());
        assert!(matches!(
            service.delete_device(&device.id).await,
            Err(PushError::UnknownDevice(_))
        ));
    }

    #[tokio::test]
    async fn the_public_key_is_created_on_first_use_and_never_changes() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("push-vapid.key");
        let service = service_in(&dir, FAST_RETRY);
        assert!(!key_file.exists(), "no key before it is needed");

        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let service = Arc::clone(&service);
                crate::util::spawn_in_span(async move { service.public_key().await.unwrap() })
            })
            .collect();
        let mut keys = HashSet::new();
        for task in tasks {
            keys.insert(task.await.unwrap());
        }
        assert_eq!(keys.len(), 1, "concurrent first use creates one key");
        let created = std::fs::read(&key_file).unwrap();

        // A new service over the same directory reads the key back.
        let restarted = service_in(&dir, FAST_RETRY);
        assert_eq!(
            restarted.public_key().await.unwrap(),
            keys.into_iter().next().unwrap()
        );
        assert_eq!(std::fs::read(&key_file).unwrap(), created);
    }

    #[tokio::test]
    async fn a_test_notification_reaches_the_push_service_encrypted_for_the_device() {
        let server = MockServer::start().await;
        accepts(&server, 201).await;
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let browser = Browser::new();
        let device = service
            .upsert_device(put(
                &format!("{}/push/laptop", server.uri()),
                &browser,
                "Laptop",
            ))
            .await
            .unwrap();

        let result = service.send_test(&device.id, 7).await.unwrap();

        assert_eq!(
            result,
            PushTestResult {
                delivered: true,
                error: None
            }
        );
        let requests = server.received_requests().await.unwrap();
        let body = &requests.first().unwrap().body;
        let payload: serde_json::Value = serde_json::from_slice(&browser.decrypt(body)).unwrap();
        assert_eq!(
            payload,
            serde_json::json!({
                "v": 1,
                "event": "test",
                "agent": "",
                "title": "Residuum test notification",
                "body": "Notifications are working on Laptop.",
                "target": "/home",
                "tag": "test",
                "badge": 7,
            })
        );
        let after = find(&service, &device.id).await.unwrap();
        assert!(after.last_success_at.is_some());
        assert!(after.last_failure.is_none());
    }

    #[tokio::test]
    async fn a_test_to_an_unknown_device_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        assert!(matches!(
            service.send_test("nope", 0).await,
            Err(PushError::UnknownDevice(_))
        ));
    }

    #[tokio::test]
    async fn a_test_that_fails_is_reported_and_tried_once() {
        let server = MockServer::start().await;
        accepts(&server, 503).await;
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = register(&service, &server, "Laptop").await;

        let result = service.send_test(&device.id, 0).await.unwrap();

        assert!(!result.delivered);
        assert!(result.error.unwrap().contains("503"));
        tokio::time::sleep(FAST_RETRY * 4).await;
        assert_eq!(requests_to(&server).await, 1, "a test is never retried");
        let after = find(&service, &device.id).await.unwrap();
        assert_eq!(after.last_failure.unwrap().status, Some(503));
    }

    #[tokio::test]
    async fn a_404_or_410_removes_the_device_and_says_so() {
        for status in [404, 410] {
            let server = MockServer::start().await;
            accepts(&server, status).await;
            let dir = tempfile::tempdir().unwrap();
            let service = service_in(&dir, FAST_RETRY);
            let device = register(&service, &server, "Laptop").await;

            let result = service.send_test(&device.id, 0).await.unwrap();

            assert!(!result.delivered, "{status}");
            assert!(result.error.unwrap().contains("removed"), "{status}");
            assert!(service.devices().await.unwrap().is_empty(), "{status}");
        }
    }

    #[tokio::test]
    async fn a_503_is_retried_once_and_a_retry_that_works_leaves_no_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = register(&service, &server, "Laptop").await;

        service.notify(sample_message(PushEvent::InboxItem), HashSet::new());

        let delivered = settle(&service, &device.id, |d| {
            d.is_some_and(|d| d.last_success_at.is_some())
        })
        .await
        .unwrap();
        assert!(
            delivered.last_failure.is_none(),
            "the first failure was not recorded"
        );
        assert_eq!(requests_to(&server).await, 2);
    }

    #[tokio::test]
    async fn a_503_that_persists_is_recorded_after_a_single_retry() {
        let server = MockServer::start().await;
        accepts(&server, 503).await;
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = register(&service, &server, "Laptop").await;

        service.notify(sample_message(PushEvent::InboxItem), HashSet::new());

        let failed = settle(&service, &device.id, |d| {
            d.is_some_and(|d| d.last_failure.is_some())
        })
        .await
        .unwrap();
        let failure = failed.last_failure.unwrap();
        assert_eq!(failure.status, Some(503));
        assert!(failure.message.contains("503"), "{failure:?}");
        assert!(failed.last_success_at.is_none());
        tokio::time::sleep(FAST_RETRY * 4).await;
        assert_eq!(requests_to(&server).await, 2, "one try and one retry");
    }

    #[tokio::test]
    async fn a_refusal_is_recorded_without_a_retry() {
        let server = MockServer::start().await;
        accepts(&server, 403).await;
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = register(&service, &server, "Laptop").await;

        service.notify(sample_message(PushEvent::InboxItem), HashSet::new());

        let failed = settle(&service, &device.id, |d| {
            d.is_some_and(|d| d.last_failure.is_some())
        })
        .await
        .unwrap();
        assert_eq!(failed.last_failure.unwrap().status, Some(403));
        tokio::time::sleep(FAST_RETRY * 4).await;
        assert_eq!(requests_to(&server).await, 1);
    }

    #[tokio::test]
    async fn a_410_in_the_background_removes_the_device() {
        let server = MockServer::start().await;
        accepts(&server, 410).await;
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = register(&service, &server, "Laptop").await;

        service.notify(sample_message(PushEvent::InboxItem), HashSet::new());

        settle(&service, &device.id, |d| d.is_none()).await;
        assert_eq!(requests_to(&server).await, 1);
    }

    #[tokio::test]
    async fn an_unreachable_push_service_is_retried_then_recorded_without_a_status() {
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        // Nothing listens on port 1.
        let device = service
            .upsert_device(put("http://127.0.0.1:1/push/x", &Browser::new(), "Laptop"))
            .await
            .unwrap();

        service.notify(sample_message(PushEvent::AgentFailed), HashSet::new());

        let failed = settle(&service, &device.id, |d| {
            d.is_some_and(|d| d.last_failure.is_some())
        })
        .await
        .unwrap();
        let failure = failed.last_failure.unwrap();
        assert_eq!(failure.status, None);
        assert!(failure.message.contains("reach"), "{failure:?}");
    }

    #[tokio::test]
    async fn the_last_failure_stays_after_a_later_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        accepts(&server, 201).await;
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let device = register(&service, &server, "Laptop").await;

        assert!(!service.send_test(&device.id, 0).await.unwrap().delivered);
        assert!(service.send_test(&device.id, 0).await.unwrap().delivered);

        let after = find(&service, &device.id).await.unwrap();
        let failure = after.last_failure.unwrap();
        assert_eq!(failure.status, Some(400));
        assert!(after.last_success_at.unwrap() >= failure.at);
    }

    #[tokio::test]
    async fn notify_reaches_only_devices_that_want_the_event_and_are_not_skipped() {
        let wants = MockServer::start().await;
        let declines = MockServer::start().await;
        let present = MockServer::start().await;
        for server in [&wants, &declines, &present] {
            accepts(server, 201).await;
        }
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, FAST_RETRY);
        let wanting = register(&service, &wants, "Wants").await;
        let declining = register(&service, &declines, "Declines").await;
        let skipped = register(&service, &present, "Present").await;
        service
            .patch_device(
                &declining.id,
                PatchPushDeviceRequest {
                    label: None,
                    preferences: Some(PushPreferencesPatch {
                        inbox_item: Some(false),
                        ..PushPreferencesPatch::default()
                    }),
                },
            )
            .await
            .unwrap();

        service.notify(
            sample_message(PushEvent::InboxItem),
            HashSet::from([skipped.id.clone()]),
        );

        settle(&service, &wanting.id, |d| {
            d.is_some_and(|d| d.last_success_at.is_some())
        })
        .await;
        tokio::time::sleep(FAST_RETRY * 2).await;
        assert_eq!(requests_to(&wants).await, 1);
        assert_eq!(requests_to(&declines).await, 0, "inbox_item is off");
        assert_eq!(requests_to(&present).await, 0, "skipped as present");
    }

    #[tokio::test]
    async fn a_device_removed_while_a_retry_waits_is_not_tried_again() {
        let server = MockServer::start().await;
        accepts(&server, 503).await;
        let dir = tempfile::tempdir().unwrap();
        let service = service_in(&dir, Duration::from_millis(400));
        let device = register(&service, &server, "Laptop").await;

        service.notify(sample_message(PushEvent::InboxItem), HashSet::new());
        for _ in 0..250 {
            if requests_to(&server).await == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        service.delete_device(&device.id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;

        assert_eq!(
            requests_to(&server).await,
            1,
            "no retry for a removed device"
        );
    }

    #[tokio::test]
    async fn the_contact_becomes_the_sub_claim_and_falls_back_to_the_project() {
        let server = MockServer::start().await;
        accepts(&server, 201).await;
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(PushService::build(
            dir.path(),
            Some("mailto:bear@example.com"),
            FAST_RETRY,
        ));
        let device = register(&service, &server, "Laptop").await;

        service.send_test(&device.id, 0).await.unwrap();
        service.set_contact(Some("https://example.com/contact"));
        service.send_test(&device.id, 0).await.unwrap();
        service.set_contact(None);
        service.send_test(&device.id, 0).await.unwrap();

        let subjects: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| {
                let header = request
                    .headers
                    .get("authorization")
                    .unwrap()
                    .to_str()
                    .unwrap();
                let jwt = header
                    .strip_prefix("vapid t=")
                    .and_then(|rest| rest.split_once(", k="))
                    .map(|(jwt, _)| jwt)
                    .unwrap();
                let claims = jwt.split('.').nth(1).unwrap();
                let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(claims)
                    .unwrap();
                let claims: serde_json::Value = serde_json::from_slice(&claims).unwrap();
                claims.get("sub").unwrap().as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(
            subjects,
            [
                "mailto:bear@example.com",
                "https://example.com/contact",
                DEFAULT_CONTACT
            ]
        );
    }

    #[tokio::test]
    async fn an_unreadable_devices_file_stops_changes_and_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("push-devices.json");
        std::fs::write(&file, "garbage").unwrap();
        let service = service_in(&dir, FAST_RETRY);

        assert!(matches!(service.devices().await, Err(PushError::Failed(_))));
        assert!(matches!(
            service
                .upsert_device(put("https://push.example/a", &Browser::new(), "x"))
                .await,
            Err(PushError::Failed(_))
        ));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "garbage");
    }

    #[test]
    fn a_subscription_must_be_https_with_well_formed_keys() {
        let browser = Browser::new();
        let sub = |endpoint: &str| WebPushSubscription {
            endpoint: endpoint.to_string(),
            keys: browser.keys(),
        };
        assert!(validate_subscription(&sub("https://fcm.googleapis.com/fcm/send/abc")).is_ok());
        for bad in ["http://push.example/a", "not a url", "ftp://push.example/a"] {
            assert!(
                matches!(
                    validate_subscription(&sub(bad)),
                    Err(PushError::BadRequest(_))
                ),
                "{bad}"
            );
        }
        let mut bad_keys = sub("https://push.example/a");
        bad_keys.keys.auth = "x".to_string();
        assert!(matches!(
            validate_subscription(&bad_keys),
            Err(PushError::BadRequest(_))
        ));
    }
}
