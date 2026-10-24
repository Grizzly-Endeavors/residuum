//! The hub-lifetime handle to remote access. The manager itself is rebuilt
//! whenever the tunnel restarts; the status channel, the sibling keys and this
//! slot outlive it, so the HTTP API and the A2A listener always have something
//! to ask.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use residuum_config::CloudConfig;
use tokio::sync::watch;

use super::engine::EngineRouters;
use super::manager::{ActionError, Notifier, RemoteAccess, RemoteAccessInputs, ResetError};
use super::siblings::SiblingKeys;
use super::siblings::discovery::SecureDiscovery;
use super::status::{RemoteAccessState, RemoteAccessStatus};
use crate::pairing::DevicePairing;
use crate::tunnel::v2::SessionHandler;
use crate::tunnel::{AgentInfo, TunnelStatus};

/// Where sibling discovery learns what the secure tunnel offers it.
pub(crate) type DiscoverySender = Arc<watch::Sender<Option<Arc<SecureDiscovery>>>>;

/// What the status says while no `[cloud]` section is configured.
const NOT_SET_UP: &str = "Residuum Cloud isn't set up on this install.";

/// What one run of the secure tunnel is started with.
pub struct TunnelStart {
    /// The `[cloud]` section the tunnel connects with.
    pub cloud: CloudConfig,
    /// The hub's config directory, under which remote access keeps its state.
    pub hub_dir: PathBuf,
    /// The A2A listener's port, when A2A is enabled.
    pub a2a_port: Option<u16>,
    /// The main gateway router, served in-process for the UI host.
    pub ui_router: axum::Router,
    /// The workbench router, served in-process for the workbench host.
    pub workbench_router: axum::Router,
    /// The port of each agent's Teams listener, by agent name.
    pub teams_ports: watch::Receiver<BTreeMap<String, u16>>,
    /// The paired devices, which the device gate checks remote requests against.
    pub pairing: DevicePairing,
    /// Where the tunnel reports its connection.
    pub tunnel_status: Arc<watch::Sender<TunnelStatus>>,
    /// Delivers a warning to the user.
    pub notify: Notifier,
    /// The hub's full agent list, sent to the relay after every connect and
    /// again on every change.
    pub agents: watch::Receiver<Vec<AgentInfo>>,
    /// Stops the tunnel once it holds `true`.
    pub shutdown: watch::Receiver<bool>,
}

/// Where the HTTP API reaches the running `RemoteAccess`.
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
        status.detail = Some(NOT_SET_UP.to_string());
        let dir = residuum_config::HubPaths::new(hub_dir).remote_access_dir();
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

    /// What sibling discovery uses while the secure tunnel is up, `None` while
    /// it isn't.
    #[must_use]
    pub fn discovery_receiver(&self) -> watch::Receiver<Option<Arc<SecureDiscovery>>> {
        self.discovery.subscribe()
    }

    /// Build the secure tunnel's manager for `start.cloud`, make it the
    /// running instance, and return the tunnel client that serves it, which
    /// runs until `start.shutdown` holds `true`. `None` when the manager can't
    /// be built: that is logged, and the status reports remote access as
    /// failed.
    #[must_use]
    pub fn start_tunnel(
        &self,
        start: TunnelStart,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let TunnelStart {
            cloud,
            hub_dir,
            a2a_port,
            ui_router,
            workbench_router,
            teams_ports,
            pairing,
            tunnel_status,
            notify,
            agents,
            shutdown,
        } = start;
        let built = RemoteAccess::new(RemoteAccessInputs {
            settings: cloud.remote.clone(),
            hub_dir,
            routers: EngineRouters {
                ui: ui_router,
                workbench: workbench_router,
            },
            a2a_port,
            teams_ports,
            pairing,
            tunnel_status: Arc::clone(&tunnel_status),
            status: self.status_sender(),
            relay_origin: crate::tunnel::v2::web_origin(&cloud.relay_url),
            notify,
            siblings: self.sibling_keys(),
            discovery: self.discovery_sender(),
            sibling_channel: None,
        });
        match built {
            Ok(remote) => {
                remote.start_background();
                self.install(remote.clone());
                let handler: Arc<dyn SessionHandler> = Arc::new(remote);
                Some(crate::tunnel::start_tunnel(
                    cloud,
                    agents,
                    shutdown,
                    tunnel_status,
                    handler,
                ))
            }
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "couldn't start the secure tunnel; Residuum Cloud is not connected");
                self.clear(
                    RemoteAccessState::Error,
                    "Residuum couldn't start the secure tunnel, so Residuum Cloud is not connected. Check Residuum's logs.",
                );
                None
            }
        }
    }

    /// Forget the running instance and report that Residuum Cloud isn't set up.
    pub fn disable(&self) {
        self.clear(RemoteAccessState::Disabled, NOT_SET_UP);
    }

    /// Make `remote` the running instance.
    pub(crate) fn install(&self, remote: RemoteAccess) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(remote);
    }

    /// Forget the running instance and report remote access as `state`.
    fn clear(&self, state: RemoteAccessState, detail: &str) {
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

    /// Ask the pin service to email a reset link. Returns the masked address.
    ///
    /// # Errors
    /// Returns a plain-language reason the email wasn't requested.
    pub async fn email_reset(&self) -> Result<String, ResetError> {
        match self.current() {
            Some(remote) => remote.email_reset().await,
            None => Err(ResetError::NotConnected),
        }
    }

    /// Cancel the reset by email that is waiting to take effect.
    ///
    /// # Errors
    /// Returns a plain-language reason it wasn't cancelled.
    pub async fn cancel_reset(&self) -> Result<(), ActionError> {
        self.running()?.cancel_reset().await
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
