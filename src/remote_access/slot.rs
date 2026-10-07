//! The hub-lifetime handle to remote access. The manager itself is rebuilt
//! whenever the tunnel restarts; the status channel, the sibling keys and this
//! slot outlive it, so the HTTP API and the A2A listener always have something
//! to ask.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::watch;

use super::manager::{ActionError, RemoteAccess, ResetError};
use super::siblings::SiblingKeys;
use super::siblings::discovery::SecureDiscovery;
use super::status::{RemoteAccessState, RemoteAccessStatus};

/// Where sibling discovery learns what the secure tunnel offers it.
pub(crate) type DiscoverySender = Arc<watch::Sender<Option<Arc<SecureDiscovery>>>>;

/// Where the HTTP API reaches the running [`RemoteAccess`].
#[derive(Clone)]
pub struct RemoteAccessSlot {
    status: Arc<watch::Sender<RemoteAccessStatus>>,
    current: Arc<Mutex<Option<RemoteAccess>>>,
    siblings: Arc<SiblingKeys>,
    discovery: DiscoverySender,
}

impl RemoteAccessSlot {
    /// A slot with remote access disabled, whose sibling keys live in the
    /// remote access directory under `hub_dir`.
    #[must_use]
    pub fn new(hub_dir: &Path) -> Self {
        let mut status = RemoteAccessStatus::new(RemoteAccessState::Disabled);
        status.detail = Some("Residuum Cloud isn't set up on this install.".to_string());
        let dir = crate::config::HubPaths::new(hub_dir).remote_access_dir();
        Self {
            status: Arc::new(watch::channel(status).0),
            current: Arc::new(Mutex::new(None)),
            siblings: Arc::new(SiblingKeys::open(&dir)),
            discovery: Arc::new(watch::channel(None).0),
        }
    }

    pub(crate) fn status_sender(&self) -> Arc<watch::Sender<RemoteAccessStatus>> {
        Arc::clone(&self.status)
    }

    /// The keys of joined siblings, which the A2A listener checks callers against.
    #[must_use]
    pub fn sibling_keys(&self) -> Arc<SiblingKeys> {
        Arc::clone(&self.siblings)
    }

    pub(crate) fn discovery_sender(&self) -> DiscoverySender {
        Arc::clone(&self.discovery)
    }

    pub(crate) fn discovery_receiver(&self) -> watch::Receiver<Option<Arc<SecureDiscovery>>> {
        self.discovery.subscribe()
    }

    /// Make `remote` the running instance.
    pub(crate) fn install(&self, remote: RemoteAccess) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(remote);
    }

    /// Forget the running instance and report remote access as `state`.
    pub(crate) fn clear(&self, state: RemoteAccessState, detail: &str) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.discovery.send_replace(None);
        self.status.send_replace(RemoteAccessStatus {
            detail: Some(detail.to_string()),
            ..RemoteAccessStatus::new(state)
        });
    }

    fn current(&self) -> Option<RemoteAccess> {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the hub is on the secure tunnel (or connecting to it). The older
    /// tunnel's relay attestation of a calling sibling is only believed while
    /// this is false.
    #[must_use]
    pub fn on_secure_tunnel(&self) -> bool {
        !matches!(
            self.status.borrow().state,
            RemoteAccessState::Disabled | RemoteAccessState::Legacy
        )
    }

    /// The status. The recovery code waiting to be saved is included only
    /// when `include_recovery_code`, which the API sets for requests made on
    /// the machine Residuum runs on.
    #[must_use]
    pub fn status(&self, include_recovery_code: bool) -> RemoteAccessStatus {
        let mut status = self.status.borrow().clone();
        let current = self.current();
        if include_recovery_code {
            status.recovery_code = current
                .as_ref()
                .and_then(RemoteAccess::pending_recovery_code);
        }
        if let Some(remote) = current {
            status.pending_joins = remote.pending_joins(&status.instances);
        }
        status
    }

    /// Look again now, instead of waiting for the next scheduled check.
    pub fn retry_now(&self) {
        if let Some(remote) = self.current() {
            remote.retry_now();
        }
    }

    /// The recovery code has been saved.
    ///
    /// # Errors
    /// Returns an error if the change couldn't be saved.
    pub async fn acknowledge_recovery_code(&self) -> anyhow::Result<()> {
        match self.current() {
            Some(remote) => remote.acknowledge_recovery_code().await,
            None => Ok(()),
        }
    }

    /// Replace every pin with this instance's account, using the recovery code.
    ///
    /// # Errors
    /// Returns a plain-language reason the reset didn't happen.
    pub async fn reset_pins(&self, recovery_code: &str) -> Result<(), ResetError> {
        match self.current() {
            Some(remote) => remote.reset_pins(recovery_code).await,
            None => Err(ResetError::NotConnected),
        }
    }

    fn running(&self) -> Result<RemoteAccess, ActionError> {
        self.current().ok_or(ActionError::NotConnected)
    }

    /// Ask the instance `instance` to approve this one as a sibling.
    ///
    /// # Errors
    /// Returns a plain-language reason the join didn't start.
    pub async fn start_join(&self, instance: &str) -> Result<(), ActionError> {
        self.running()?.start_join(instance).await
    }

    /// Approve the join request waiting under `id`.
    ///
    /// # Errors
    /// Returns a plain-language reason the approval didn't happen.
    pub async fn approve_join(&self, id: &str) -> Result<(), ActionError> {
        self.running()?.approve_join(id).await
    }

    /// Deny the join request waiting under `id`.
    ///
    /// # Errors
    /// Returns a plain-language reason the request wasn't denied.
    pub fn deny_join(&self, id: &str) -> Result<(), ActionError> {
        self.running()?.deny_join(id)
    }

    /// Remove the pin of `account_uri`, whose instance is no longer in the relay's list.
    ///
    /// # Errors
    /// Returns a plain-language reason the pin wasn't removed.
    pub async fn remove_pin(&self, account_uri: &str) -> Result<(), ActionError> {
        self.running()?.remove_pin(account_uri).await
    }

    /// Make `slug` the instance the user's address goes to.
    ///
    /// # Errors
    /// Returns a plain-language reason the switch wasn't requested.
    pub fn activate_instance(&self, slug: &str) -> Result<(), ActionError> {
        self.running()?.activate_instance(slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relays_sibling_attestation_counts_only_off_the_secure_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        let slot = RemoteAccessSlot::new(dir.path());
        assert!(
            !slot.on_secure_tunnel(),
            "disabled: the older tunnel, if any"
        );
        slot.status_sender()
            .send_modify(|s| s.state = RemoteAccessState::Legacy);
        assert!(!slot.on_secure_tunnel());
        for state in [
            RemoteAccessState::Connecting,
            RemoteAccessState::NeedsJoin,
            RemoteAccessState::Ready,
            RemoteAccessState::Error,
        ] {
            slot.status_sender().send_modify(|s| s.state = state);
            assert!(slot.on_secure_tunnel(), "{state:?}");
        }
    }

    #[tokio::test]
    async fn actions_without_a_running_manager_say_the_tunnel_isnt_connected() {
        let dir = tempfile::tempdir().unwrap();
        let slot = RemoteAccessSlot::new(dir.path());
        assert!(matches!(
            slot.start_join("laptop").await,
            Err(ActionError::NotConnected)
        ));
        assert!(matches!(
            slot.activate_instance("laptop"),
            Err(ActionError::NotConnected)
        ));
        assert!(matches!(
            slot.deny_join("x"),
            Err(ActionError::NotConnected)
        ));
    }
}
