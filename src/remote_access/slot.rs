//! The hub-lifetime handle to remote access. The manager itself is rebuilt
//! whenever the tunnel restarts; the status channel and this slot outlive it,
//! so the HTTP API always has something to ask.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::watch;

use super::manager::{RemoteAccess, ResetError};
use super::status::{RemoteAccessState, RemoteAccessStatus};

/// Where the HTTP API reaches the running [`RemoteAccess`].
#[derive(Clone)]
pub struct RemoteAccessSlot {
    status: Arc<watch::Sender<RemoteAccessStatus>>,
    current: Arc<Mutex<Option<RemoteAccess>>>,
}

impl Default for RemoteAccessSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteAccessSlot {
    /// A slot with remote access disabled.
    #[must_use]
    pub fn new() -> Self {
        let mut status = RemoteAccessStatus::new(RemoteAccessState::Disabled);
        status.detail = Some("Residuum Cloud isn't set up on this install.".to_string());
        Self {
            status: Arc::new(watch::channel(status).0),
            current: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn status_sender(&self) -> Arc<watch::Sender<RemoteAccessStatus>> {
        Arc::clone(&self.status)
    }

    /// Make `remote` the running instance.
    pub(crate) fn install(&self, remote: RemoteAccess) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(remote);
    }

    /// Forget the running instance and report remote access as `state`.
    pub(crate) fn clear(&self, state: RemoteAccessState, detail: &str) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = None;
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

    /// The status. The recovery code waiting to be saved is included only
    /// when `include_recovery_code`, which the API sets for requests made on
    /// the machine Residuum runs on.
    #[must_use]
    pub fn status(&self, include_recovery_code: bool) -> RemoteAccessStatus {
        let mut status = self.status.borrow().clone();
        if include_recovery_code {
            status.recovery_code = self.current().and_then(|r| r.pending_recovery_code());
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
}
