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

pub(crate) type DiscoverySender = Arc<watch::Sender<Option<Arc<SecureDiscovery>>>>;

const NOT_SET_UP: &str = "Residuum Cloud isn't set up on this install.";

pub struct TunnelStart {
    pub cloud: CloudConfig,
    pub hub_dir: PathBuf,
    pub a2a_port: Option<u16>,
    pub ui_router: axum::Router,
    pub workbench_router: axum::Router,
    pub teams_ports: watch::Receiver<BTreeMap<String, u16>>,
    pub pairing: DevicePairing,
    pub tunnel_status: Arc<watch::Sender<TunnelStatus>>,
    pub notify: Notifier,
    pub agents: watch::Receiver<Vec<AgentInfo>>,
    pub shutdown: watch::Receiver<bool>,
}

#[derive(Clone)]
pub struct RemoteAccessSlot {
    status: Arc<watch::Sender<RemoteAccessStatus>>,
    current: Arc<Mutex<Option<RemoteAccess>>>,
    siblings: Arc<SiblingKeys>,
    discovery: DiscoverySender,
}

impl RemoteAccessSlot {
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

    #[must_use]
    pub fn sibling_keys(&self) -> Arc<SiblingKeys> {
        Arc::clone(&self.siblings)
    }

    pub(crate) fn discovery_sender(&self) -> DiscoverySender {
        Arc::clone(&self.discovery)
    }

    #[must_use]
    pub fn discovery_receiver(&self) -> watch::Receiver<Option<Arc<SecureDiscovery>>> {
        self.discovery.subscribe()
    }

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

    pub fn disable(&self) {
        self.clear(RemoteAccessState::Disabled, NOT_SET_UP);
    }

    pub(crate) fn install(&self, remote: RemoteAccess) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some(remote);
    }

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

    pub fn retry_now(&self) {
        if let Some(remote) = self.current() {
            remote.retry_now();
        }
    }

    pub async fn acknowledge_recovery_code(&self) -> anyhow::Result<()> {
        match self.current() {
            Some(remote) => remote.acknowledge_recovery_code().await,
            None => Ok(()),
        }
    }

    pub async fn reset_pins(&self, recovery_code: &str) -> Result<(), ResetError> {
        match self.current() {
            Some(remote) => remote.reset_pins(recovery_code).await,
            None => Err(ResetError::NotConnected),
        }
    }

    pub async fn email_reset(&self) -> Result<String, ResetError> {
        match self.current() {
            Some(remote) => remote.email_reset().await,
            None => Err(ResetError::NotConnected),
        }
    }

    pub async fn cancel_reset(&self) -> Result<(), ActionError> {
        self.running()?.cancel_reset().await
    }

    fn running(&self) -> Result<RemoteAccess, ActionError> {
        self.current().ok_or(ActionError::NotConnected)
    }

    pub async fn start_join(&self, instance: &str) -> Result<(), ActionError> {
        self.running()?.start_join(instance).await
    }

    pub async fn approve_join(&self, id: &str) -> Result<(), ActionError> {
        self.running()?.approve_join(id).await
    }

    pub fn deny_join(&self, id: &str) -> Result<(), ActionError> {
        self.running()?.deny_join(id)
    }

    pub async fn remove_pin(&self, account_uri: &str) -> Result<(), ActionError> {
        self.running()?.remove_pin(account_uri).await
    }

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
