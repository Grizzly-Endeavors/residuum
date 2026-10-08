//! The remote-access manager: it decides, for each tunnel v2 session, whether
//! this install may serve remotely and walks it there: verify the identity the
//! relay announces, enroll with the pin service, wait for the DNS record that
//! lets this instance's account issue certificates, order the certificate, and
//! keep it renewed.
//!
//! The manager implements [`SessionHandler`], so the tunnel client calls it
//! for every connect, stream and disconnect. It owns every trust decision:
//! nothing the relay sends is believed beyond what it can prove (see
//! `super::identity`).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use super::acme::{AcmeAccount, AcmeSettings, CertStore, CertificateOrder, renewal_window};
use super::caa::{CaaSettings, CaaWaitError, wait_for_caa};
use super::engine::{Engine, EngineDeps, EngineRouters};
use super::identity::{IdentityError, is_valid_slug, verify_announcement};
use super::jws::AccountSigner;
use super::pins::{Enrollment, Pin, PinClient, PinError, generate_recovery_code, is_recovery_code};
use super::siblings::SiblingKeys;
use super::siblings::client::{HttpChannel, SiblingChannel};
use super::siblings::discovery::SecureDiscovery;
use super::siblings::host::{HostContext, JoinHost};
use super::siblings::routes;
use super::siblings::service::{ApproveError, Approver, Joiner, SiblingService};
use super::slot::DiscoverySender;
use super::status::{
    CertificateInfo, InstanceInfo, JoinProgress, JoinState, PendingJoinInfo, PinInfo,
    RemoteAccessState, RemoteAccessStatus, RemoteHosts, SiblingInfo,
};
use super::store::{LocalIdentity, StateStore};
use super::tls::{CertBundle, CertResolver};
use super::types::Hostnames;
use crate::config::RemoteAccessSettings;
use crate::pairing::{DevicePairing, Identity};
use crate::tunnel::TunnelStatus;
use crate::tunnel::v2::frames::InstanceSummary;
use crate::tunnel::v2::{
    ClaimError, ConnectedInfo, IncomingStream, RelayLink, SessionHandler, Verdict,
};

/// The most characters of an instance's display name kept from the relay.
const MAX_INSTANCE_NAME: usize = 64;
/// How long the DNS record for the account may take to show up publicly.
const CAA_WAIT: Duration = Duration::from_mins(10);
const CAA_POLL: Duration = Duration::from_secs(10);
/// How often the pin set is read to look for accounts nobody here agreed to.
const PIN_CHECK_INTERVAL: Duration = Duration::from_hours(1);
/// How often a waiting join or a healthy certificate is looked at again.
const NEEDS_JOIN_RECHECK: Duration = Duration::from_mins(5);
const ARI_RECHECK: Duration = Duration::from_hours(6);
const MAX_RETRY: Duration = Duration::from_mins(15);
const CLAIM_ATTEMPTS: u32 = 12;
const CLAIM_RETRY: Duration = Duration::from_secs(5);

/// Where plain-language warnings for the person go.
pub(crate) type Notifier = Arc<dyn Fn(String) + Send + Sync>;

/// Everything the manager is built from.
pub(crate) struct RemoteAccessInputs {
    pub(crate) settings: RemoteAccessSettings,
    pub(crate) hub_dir: PathBuf,
    pub(crate) routers: EngineRouters,
    pub(crate) a2a_port: Option<u16>,
    pub(crate) teams_ports: watch::Receiver<std::collections::BTreeMap<String, u16>>,
    pub(crate) pairing: DevicePairing,
    pub(crate) tunnel_status: Arc<watch::Sender<TunnelStatus>>,
    pub(crate) status: Arc<watch::Sender<RemoteAccessStatus>>,
    pub(crate) notify: Notifier,
    /// The keys of joined siblings, shared with the A2A listener.
    pub(crate) siblings: Arc<SiblingKeys>,
    /// Where sibling discovery hears about the directory and the joined siblings.
    pub(crate) discovery: DiscoverySender,
    /// How join requests reach a sibling. `None` uses HTTPS to the sibling's
    /// derived host.
    pub(crate) sibling_channel: Option<Arc<dyn SiblingChannel>>,
}

struct Session {
    link: RelayLink,
    user: String,
    slug: String,
    /// What lets this connection see private agents in the apex directory.
    directory_token: String,
    task: JoinHandle<()>,
}

struct Inner {
    settings: RemoteAccessSettings,
    store: StateStore,
    resolver: Arc<CertResolver>,
    cert_store: CertStore,
    acme: AcmeSettings,
    caa: CaaSettings,
    pin_client: PinClient,
    account: tokio::sync::Mutex<Option<Arc<AcmeAccount>>>,
    engine: Arc<Engine>,
    hostnames: watch::Sender<Option<Hostnames>>,
    status: Arc<watch::Sender<RemoteAccessStatus>>,
    pairing: DevicePairing,
    tunnel_status: Arc<watch::Sender<TunnelStatus>>,
    notify: Notifier,
    session: Mutex<Option<Session>>,
    /// Wakes the session's driver early: a retry or a finished reset.
    kick: Notify,
    alerted_pins: Mutex<HashSet<String>>,
    siblings: SiblingService,
    discovery: DiscoverySender,
    /// The join this instance started, while it runs.
    join_task: Mutex<Option<JoinHandle<()>>>,
}

/// Remote access for this install.
#[derive(Clone)]
pub(crate) struct RemoteAccess {
    inner: Arc<Inner>,
}

impl RemoteAccess {
    /// Open the stored state and build the engine. Nothing is served until a
    /// session connects and its identity checks out.
    ///
    /// # Errors
    /// Returns an error if the pin service client can't be built.
    pub(crate) fn new(inputs: RemoteAccessInputs) -> anyhow::Result<Self> {
        let RemoteAccessInputs {
            settings,
            hub_dir,
            routers,
            a2a_port,
            teams_ports,
            pairing,
            tunnel_status,
            status,
            notify,
            siblings,
            discovery,
            sibling_channel,
        } = inputs;
        let dir = crate::config::HubPaths::new(&hub_dir).remote_access_dir();
        let store = StateStore::open(&dir);
        let resolver = CertResolver::new();
        let acme = AcmeSettings {
            directory_url: settings.acme_directory.clone(),
            root_ca_pem: settings.acme_root_ca.clone(),
            state_dir: dir.clone(),
        };
        let cert_store = CertStore::new(&dir, &settings.acme_directory);
        let (hostnames, hostnames_rx) = watch::channel(None);
        let channel: Arc<dyn SiblingChannel> = match sibling_channel {
            Some(channel) => channel,
            None => Arc::new(HttpChannel::https(settings.acme_root_ca.as_deref())?),
        };
        let join_host = Arc::new(JoinHost::default());
        let engine = Engine::new(EngineDeps {
            resolver: Arc::clone(&resolver),
            hostnames: hostnames_rx,
            routers,
            sibling_routes: routes::router(Arc::clone(&join_host)),
            a2a_port,
            teams_ports,
        });
        let inner = Arc::new(Inner {
            caa: CaaSettings {
                resolver: settings.caa_resolver,
            },
            pin_client: PinClient::new(&settings.pin_service_url)?,
            settings,
            store,
            resolver,
            cert_store,
            acme,
            account: tokio::sync::Mutex::new(None),
            engine,
            hostnames,
            status,
            pairing,
            tunnel_status,
            notify,
            session: Mutex::new(None),
            kick: Notify::new(),
            alerted_pins: Mutex::new(HashSet::new()),
            siblings: SiblingService::new(join_host, siblings, channel),
            discovery,
            join_task: Mutex::new(None),
        });
        inner.load_stored();
        Ok(Self { inner })
    }

    /// Start the work that doesn't depend on a session: the pairing identity
    /// and the hourly look at the pin service.
    pub(crate) fn start_background(&self) {
        // Weak, so a manager that has been replaced (the tunnel restarted)
        // lets its watcher end instead of living on.
        let inner = Arc::downgrade(&self.inner);
        crate::util::spawn_monitored("remote-access-pins", async move {
            if let Some(strong) = inner.upgrade()
                && let Some(identity) = strong.store.identity()
            {
                strong.apply_pairing_identity(&identity).await;
            }
            loop {
                let Some(strong) = inner.upgrade() else { break };
                strong.refresh_pins().await;
                drop(strong);
                tokio::time::sleep(PIN_CHECK_INTERVAL).await;
            }
        });
    }

    /// The recovery code waiting to be saved, if any.
    pub(crate) fn pending_recovery_code(&self) -> Option<String> {
        self.inner.store.pending_recovery_code()
    }

    /// Ask the session's driver to look again now.
    pub(crate) fn retry_now(&self) {
        self.inner.kick.notify_one();
    }

    /// The person has saved the recovery code; forget it.
    ///
    /// # Errors
    /// Returns an error if the change couldn't be saved.
    pub(crate) async fn acknowledge_recovery_code(&self) -> anyhow::Result<()> {
        self.inner.store.set_pending_recovery_code(None).await?;
        self.inner.status.send_modify(|status| {
            status.recovery_code_pending = false;
        });
        Ok(())
    }

    /// The join requests waiting for a decision, with `instances` (the
    /// relay's list) as the hint for each.
    pub(crate) fn pending_joins(&self, instances: &[InstanceInfo]) -> Vec<PendingJoinInfo> {
        self.inner.pending_joins(instances)
    }

    pub(crate) async fn start_join(&self, target: &str) -> Result<(), ActionError> {
        self.inner.start_join(target).await
    }

    pub(crate) async fn approve_join(&self, id: &str) -> Result<(), ActionError> {
        self.inner.approve_join(id).await
    }

    pub(crate) fn deny_join(&self, id: &str) -> Result<(), ActionError> {
        self.inner.deny_join(id)
    }

    pub(crate) async fn remove_pin(&self, account_uri: &str) -> Result<(), ActionError> {
        self.inner.remove_pin(account_uri).await
    }

    pub(crate) fn activate_instance(&self, slug: &str) -> Result<(), ActionError> {
        self.inner.activate_instance(slug)
    }

    /// Replace every pin with this instance's account, proving ownership with
    /// the recovery code. Returns the new recovery code.
    ///
    /// # Errors
    /// Returns a plain-language reason the reset didn't happen.
    pub(crate) async fn reset_pins(&self, recovery_code: &str) -> Result<(), ResetError> {
        self.inner.reset_pins(recovery_code.trim()).await
    }
}

/// Why a pin reset didn't happen.
#[derive(Debug, thiserror::Error)]
pub enum ResetError {
    /// The input isn't acceptable; the message says why.
    #[error("{0}")]
    Invalid(String),
    /// The relay isn't connected on the secure tunnel.
    #[error("Residuum Cloud isn't connected on the secure tunnel right now. Try again once it is.")]
    NotConnected,
    /// The grant or the pin service refused, or failed.
    #[error("{0}")]
    Failed(String),
}

/// Why a sibling, pin or instance action didn't happen.
#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    /// The input isn't acceptable; the message says why.
    #[error("{0}")]
    Invalid(String),
    /// The relay isn't connected on the secure tunnel.
    #[error("Residuum Cloud isn't connected on the secure tunnel right now. Try again once it is.")]
    NotConnected,
    /// The action was refused or failed; the message says why.
    #[error("{0}")]
    Failed(String),
}

/// Whether a pin of `slug` may be removed from the UI: not this instance's own,
/// and the relay's list (when known) doesn't include its instance.
fn is_removable(own: bool, slug: &str, instances: &[InstanceInfo]) -> bool {
    !own && !instances.is_empty() && !instances.iter().any(|i| i.slug == slug)
}

impl Inner {
    fn lock_session(&self) -> MutexGuard<'_, Option<Session>> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Load what an earlier run stored: the identity, and the certificate.
    fn load_stored(&self) {
        if let Some(identity) = self.store.identity() {
            self.apply_identity_sync(&identity);
        }
        if let Some(bundle) = self.cert_store.load() {
            match self.resolver.set_certificate(&bundle) {
                Ok(()) => self.set_certificate_status(&bundle),
                Err(e) => {
                    tracing::error!(error = %format!("{e:#}"), "the stored certificate can't be used; a new one will be ordered");
                }
            }
        }
        self.status.send_modify(|status| {
            status.recovery_code_pending = self.store.pending_recovery_code().is_some();
        });
        self.refresh_sibling_status();
    }

    fn base(&self) -> &str {
        &self.settings.base_domain
    }

    fn apply_identity_sync(&self, identity: &LocalIdentity) {
        let hosts = Hostnames::derive(&identity.user, &identity.slug, self.base());
        self.hostnames.send_replace(Some(hosts.clone()));
        let (ui, workbench, instance_origin) = (
            format!("https://{}", hosts.ui),
            format!("https://{}", hosts.workbench),
            format!("https://{}", hosts.instance),
        );
        self.status.send_modify(|status| {
            status.user = Some(identity.user.clone());
            status.slug = Some(identity.slug.clone());
            status.hosts = Some(RemoteHosts {
                ui: hosts.ui.clone(),
                workbench: hosts.workbench.clone(),
                instance: hosts.instance.clone(),
            });
        });
        let slug = identity.slug.clone();
        self.tunnel_status.send_modify(|status| {
            if let TunnelStatus::Connected {
                origin,
                workbench_origin,
                instance,
                instance_origin: announced_instance_origin,
                ..
            } = status
            {
                *origin = Some(ui.clone());
                *workbench_origin = Some(workbench.clone());
                *instance = Some(slug.clone());
                *announced_instance_origin = Some(instance_origin.clone());
            }
        });
    }

    async fn apply_pairing_identity(&self, identity: &LocalIdentity) {
        let hosts = Hostnames::derive(&identity.user, &identity.slug, self.base());
        let local = Identity {
            slug: Some(identity.slug.clone()),
            ui_origin: Some(format!("https://{}", hosts.ui)),
            workbench_origin: Some(format!("https://{}", hosts.workbench)),
        };
        if let Err(e) = self.pairing.set_local_identity(local).await {
            tracing::warn!(error = %e, "couldn't save this instance's address for pairing links");
        }
    }

    fn set_state(&self, state: RemoteAccessState, detail: Option<String>) {
        self.status.send_modify(|status| {
            status.state = state;
            status.detail = detail;
        });
    }

    fn set_certificate_status(&self, bundle: &CertBundle) {
        self.status.send_modify(|status| {
            status.certificate = Some(CertificateInfo {
                not_after: bundle.not_after.to_rfc3339(),
                renews_at: bundle.renew_at().to_rfc3339(),
            });
        });
    }

    /// The ACME account, created on first use.
    async fn account(&self) -> anyhow::Result<Arc<AcmeAccount>> {
        let mut guard = self.account.lock().await;
        if let Some(account) = guard.as_ref() {
            return Ok(Arc::clone(account));
        }
        let account = Arc::new(AcmeAccount::load_or_create(&self.acme).await?);
        *guard = Some(Arc::clone(&account));
        Ok(account)
    }

    // ── Pins ─────────────────────────────────────────────────────────

    /// Read the pin set for the stored identity and update the status and
    /// the alerts.
    async fn refresh_pins(&self) {
        let Some(identity) = self.store.identity() else {
            return;
        };
        match self.pin_client.list(&identity.user).await {
            Ok(pins) => self.record_pins(&pins).await,
            Err(e) => {
                tracing::warn!(error = %e, "couldn't read this user's pins; unknown certificate accounts can't be checked for until it works");
            }
        }
    }

    async fn record_pins(&self, pins: &[Pin]) {
        let own = self
            .account
            .lock()
            .await
            .as_ref()
            .map(|account| account.uri().to_string());
        let instances = self.status.borrow().instances.clone();
        let infos: Vec<PinInfo> = pins
            .iter()
            .map(|pin| {
                let is_own = own.as_deref() == Some(pin.account_uri.as_str());
                PinInfo {
                    account_uri: pin.account_uri.clone(),
                    slug: pin.slug.clone(),
                    own: is_own,
                    known: self.store.is_known(&pin.account_uri),
                    removable: is_removable(is_own, &pin.slug, &instances),
                }
            })
            .collect();
        for pin in infos.iter().filter(|pin| !pin.known) {
            let first_time = self
                .alerted_pins
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(pin.account_uri.clone());
            if first_time {
                tracing::warn!(account = %pin.account_uri, slug = %pin.slug, "the pin service lists a certificate account this instance never approved");
                (self.notify)(format!(
                    "A certificate account for the instance \"{}\" is allowed to issue certificates for your Residuum addresses, and this instance never approved it. If you didn't add it, someone else may be able to get a certificate for your address. Check Settings, Remote access.",
                    pin.slug
                ));
            }
        }
        self.status.send_modify(|status| status.pins = infos);
    }

    // ── Sessions ─────────────────────────────────────────────────────

    fn start_session(
        self: &Arc<Self>,
        link: RelayLink,
        user: String,
        slug: String,
        directory_token: String,
    ) {
        let driver = Arc::clone(self);
        let (driver_link, driver_user, driver_slug) = (link.clone(), user.clone(), slug.clone());
        let task = crate::util::spawn_monitored("remote-access-session", async move {
            driver.drive(driver_link, driver_user, driver_slug).await;
        });
        if let Some(previous) = self.lock_session().replace(Session {
            link,
            user,
            slug,
            directory_token,
            task,
        }) {
            previous.task.abort();
        }
    }

    async fn drive(self: Arc<Self>, link: RelayLink, user: String, slug: String) {
        let mut failures: u32 = 0;
        let mut last_message = String::new();
        loop {
            let wait = match self.advance(&link, &user, &slug).await {
                Ok(wait) => {
                    failures = 0;
                    last_message.clear();
                    wait
                }
                Err(e) => {
                    failures = failures.saturating_add(1);
                    let message = e.to_string();
                    if message == last_message {
                        tracing::debug!(error = %format!("{e:#}"), failures, "remote access is still failing");
                    } else {
                        tracing::warn!(error = %format!("{e:#}"), "remote access setup failed; it will retry");
                        last_message.clone_from(&message);
                    }
                    self.fail(message);
                    retry_delay(failures)
                }
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = self.kick.notified() => {}
            }
        }
    }

    fn fail(&self, message: String) {
        // A valid certificate keeps serving through a failed renewal.
        let still_served = self
            .cert_store
            .load()
            .is_some_and(|bundle| bundle.not_after > Utc::now());
        if still_served {
            self.set_state(
                RemoteAccessState::Ready,
                Some(format!("Renewing the certificate failed: {message} Residuum keeps trying; the current certificate still works.")),
            );
        } else {
            self.set_state(RemoteAccessState::Error, Some(message));
        }
    }

    /// One pass: make sure this instance is pinned and has a current
    /// certificate. Returns how long to wait before looking again.
    async fn advance(&self, link: &RelayLink, user: &str, slug: &str) -> anyhow::Result<Duration> {
        let account = self
            .account()
            .await
            .map_err(|e| anyhow::anyhow!("Residuum couldn't set up its certificate account with the certificate authority: {e:#}"))?;
        let pins = self.pin_client.list(user).await.map_err(|e| {
            anyhow::anyhow!("Residuum couldn't read which certificate accounts are allowed for your address: {e}")
        })?;
        self.record_pins(&pins).await;

        let own = pins.iter().find(|pin| pin.account_uri == account.uri());
        if let Some(pin) = own {
            if pin.slug != slug {
                anyhow::bail!(
                    "this instance's certificate account is pinned for the instance \"{}\", but the relay calls this instance \"{slug}\"",
                    pin.slug
                );
            }
            self.ensure_identity(user, slug, &account).await?;
            self.discard_unused_recovery_code(&pins, &account).await;
            self.open_for_joins(user, slug, &account);
            self.publish_discovery();
        } else if pins.is_empty() {
            self.enroll(link, &account, user, slug).await?;
            self.open_for_joins(user, slug, &account);
            self.publish_discovery();
        } else {
            self.siblings.host.set_context(None);
            self.discard_unused_recovery_code(&pins, &account).await;
            self.set_state(
                RemoteAccessState::NeedsJoin,
                Some(format!(
                    "Another instance of yours is already set up for remote access ({}). This instance has to join it before it can serve remotely.",
                    pins.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>().join(", ")
                )),
            );
            return Ok(NEEDS_JOIN_RECHECK);
        }
        self.ensure_certificate(link, &account).await
    }

    async fn ensure_identity(
        &self,
        user: &str,
        slug: &str,
        account: &AcmeAccount,
    ) -> anyhow::Result<()> {
        if self.store.identity().is_some() {
            return Ok(());
        }
        let identity = LocalIdentity {
            user: user.to_string(),
            slug: slug.to_string(),
        };
        self.store
            .set_identity(identity.clone(), account.uri())
            .await?;
        self.apply_identity_sync(&identity);
        self.apply_pairing_identity(&identity).await;
        Ok(())
    }

    /// A pending recovery code belongs to an enrollment that happened. When
    /// the pin set shows it didn't, the code protects nothing.
    async fn discard_unused_recovery_code(&self, pins: &[Pin], account: &AcmeAccount) {
        if self.store.pending_recovery_code().is_some()
            && !pins.iter().any(|pin| pin.account_uri == account.uri())
        {
            if let Err(e) = self.store.set_pending_recovery_code(None).await {
                tracing::warn!(error = %format!("{e:#}"), "couldn't discard an unused recovery code");
            }
            self.status
                .send_modify(|status| status.recovery_code_pending = false);
        }
    }

    async fn enroll(
        &self,
        link: &RelayLink,
        account: &AcmeAccount,
        user: &str,
        slug: &str,
    ) -> anyhow::Result<()> {
        self.set_state(
            RemoteAccessState::Enrolling,
            Some("Setting up this instance's certificate account.".to_string()),
        );
        let recovery_code = generate_recovery_code()?;
        // Saved before the request, so a crash after the pin service commits
        // can't leave an enrollment whose recovery code was never kept.
        self.store
            .set_pending_recovery_code(Some(recovery_code.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("Residuum couldn't save the recovery code: {e:#}"))?;
        let grant = link.request_grant("enroll").await.map_err(|reason| {
            anyhow::anyhow!("the relay wouldn't let this instance enroll: {reason}")
        })?;
        let enrollment = Enrollment {
            user,
            slug,
            grant: &grant,
            recovery_code: &recovery_code,
        };
        match self.pin_client.enroll(account, &enrollment).await {
            Ok(pins) => {
                self.ensure_identity(user, slug, account).await?;
                self.record_pins(&pins).await;
                self.status
                    .send_modify(|status| status.recovery_code_pending = true);
                tracing::info!(user = %user, slug = %slug, "enrolled with the pin service");
                (self.notify)(
                    "Remote access is set up. Save the recovery code shown in Settings, Remote access: it's the only way to take your address back if every instance's certificate account is lost."
                        .to_string(),
                );
                Ok(())
            }
            Err(PinError::Rejected { status: 409, .. }) => {
                anyhow::bail!(
                    "another instance of yours enrolled first, so this instance has to join it"
                )
            }
            Err(e) => Err(anyhow::anyhow!(
                "enrolling with the pin service failed: {e}"
            )),
        }
    }

    async fn ensure_certificate(
        &self,
        link: &RelayLink,
        account: &Arc<AcmeAccount>,
    ) -> anyhow::Result<Duration> {
        let hosts = self.hostnames.borrow().clone().ok_or_else(|| {
            anyhow::anyhow!("remote access has no identity to derive host names from")
        })?;
        let names: Vec<String> = hosts.all().iter().map(|n| (*n).to_string()).collect();
        if let Some(bundle) = self.cert_store.load()
            && bundle.names == names
            && bundle.not_after > Utc::now()
        {
            let ari = renewal_window(account, &bundle).await;
            let due = ari.map_or_else(|| bundle.renew_at(), |start| start.min(bundle.renew_at()));
            if Utc::now() < due {
                self.set_state(RemoteAccessState::Ready, None);
                self.set_certificate_status(&bundle);
                let until_due = (due - Utc::now()).to_std().unwrap_or(Duration::ZERO);
                return Ok(until_due.min(ARI_RECHECK));
            }
        }

        self.set_state(
            RemoteAccessState::WaitingForDns,
            Some("Waiting for the DNS record that lets this instance's account issue certificates. This usually takes a minute or two.".to_string()),
        );
        wait_for_caa(
            &self.caa,
            &[hosts.ui.clone(), hosts.workbench.clone()],
            account.uri(),
            CAA_WAIT,
            CAA_POLL,
        )
        .await
        .map_err(|e| match e {
            CaaWaitError::TimedOut { missing } => anyhow::anyhow!(
                "the DNS record that allows this instance's certificate account hasn't appeared for {} after 10 minutes",
                missing.join(", ")
            ),
            CaaWaitError::Lookup(e) => anyhow::anyhow!("couldn't look up DNS records: {e:#}"),
        })?;

        self.claim_names(link, &names).await?;
        self.set_state(
            RemoteAccessState::Ordering,
            Some("Getting a certificate.".to_string()),
        );
        let ordered = CertificateOrder {
            account,
            resolver: &self.resolver,
            names: &names,
        }
        .run()
        .await;
        link.release_challenge(&names);
        let bundle =
            ordered.map_err(|e| anyhow::anyhow!("ordering the certificate failed: {e:#}"))?;

        self.cert_store
            .save(&bundle)
            .await
            .map_err(|e| anyhow::anyhow!("Residuum couldn't save the new certificate: {e:#}"))?;
        self.resolver
            .set_certificate(&bundle)
            .map_err(|e| anyhow::anyhow!("the new certificate can't be used: {e:#}"))?;
        self.set_certificate_status(&bundle);
        self.set_state(RemoteAccessState::Ready, None);
        tracing::info!(not_after = %bundle.not_after, renew_at = %bundle.renew_at(), "certificate installed");
        let until_due = (bundle.renew_at() - Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        Ok(until_due.min(ARI_RECHECK))
    }

    async fn claim_names(&self, link: &RelayLink, names: &[String]) -> anyhow::Result<()> {
        for attempt in 1..=CLAIM_ATTEMPTS {
            match link.claim_challenge(names).await {
                Ok(()) => return Ok(()),
                Err(ClaimError::Busy) => {
                    tracing::debug!(
                        attempt,
                        "another instance holds the challenge claim; waiting"
                    );
                    tokio::time::sleep(CLAIM_RETRY).await;
                }
                Err(ClaimError::Timeout) => {
                    anyhow::bail!(
                        "the relay didn't answer the request to route the certificate check here"
                    )
                }
                Err(ClaimError::Closed) => {
                    anyhow::bail!("the connection to the relay closed while getting a certificate")
                }
            }
        }
        anyhow::bail!(
            "another of your instances is getting a certificate right now; this one waits its turn"
        )
    }

    async fn reset_pins(&self, recovery_code: &str) -> Result<(), ResetError> {
        if !is_recovery_code(recovery_code) {
            return Err(ResetError::Invalid(
                "A recovery code is 20 letters and digits (A to Z and 2 to 7).".to_string(),
            ));
        }
        let (link, user, slug) = {
            let session = self.lock_session();
            let session = session.as_ref().ok_or(ResetError::NotConnected)?;
            (
                session.link.clone(),
                session.user.clone(),
                session.slug.clone(),
            )
        };
        let account = self.account().await.map_err(|e| {
            ResetError::Failed(format!(
                "Residuum couldn't set up its certificate account: {e:#}"
            ))
        })?;
        let new_code = generate_recovery_code().map_err(|e| {
            ResetError::Failed(format!("Residuum couldn't make a recovery code: {e:#}"))
        })?;
        self.store
            .set_pending_recovery_code(Some(new_code.clone()))
            .await
            .map_err(|e| {
                ResetError::Failed(format!(
                    "Residuum couldn't save the new recovery code: {e:#}"
                ))
            })?;
        let grant = link.request_grant("reset").await.map_err(|reason| {
            ResetError::Failed(format!("The relay wouldn't allow a reset: {reason}"))
        })?;
        let enrollment = Enrollment {
            user: &user,
            slug: &slug,
            grant: &grant,
            recovery_code: &new_code,
        };
        match self
            .pin_client
            .reset(account.as_ref(), &enrollment, recovery_code)
            .await
        {
            Ok(pins) => {
                self.store.reset_known(account.uri()).await.map_err(|e| {
                    ResetError::Failed(format!("Residuum couldn't save the reset: {e:#}"))
                })?;
                self.ensure_identity(&user, &slug, &account)
                    .await
                    .map_err(|e| {
                        ResetError::Failed(format!("Residuum couldn't save the reset: {e:#}"))
                    })?;
                self.record_pins(&pins).await;
                self.status
                    .send_modify(|status| status.recovery_code_pending = true);
                tracing::warn!(user = %user, "pins were reset with a recovery code");
                self.kick.notify_one();
                Ok(())
            }
            Err(PinError::Rejected { status: 403, .. }) => {
                self.forget_new_code().await;
                Err(ResetError::Failed(
                    "That recovery code isn't right.".to_string(),
                ))
            }
            Err(PinError::Rejected { status: 409, .. }) => {
                self.forget_new_code().await;
                Err(ResetError::Failed(
                    "There is nothing to reset: this address was never set up for remote access."
                        .to_string(),
                ))
            }
            Err(e) => {
                self.forget_new_code().await;
                Err(ResetError::Failed(format!("The reset failed: {e}")))
            }
        }
    }

    async fn forget_new_code(&self) {
        if let Err(e) = self.store.set_pending_recovery_code(None).await {
            tracing::warn!(error = %format!("{e:#}"), "couldn't discard an unused recovery code");
        }
        self.status
            .send_modify(|status| status.recovery_code_pending = false);
    }

    // ── Siblings, pins and instances ─────────────────────────────────

    /// Begin taking join requests: this instance is pinned and knows who it is.
    fn open_for_joins(&self, user: &str, slug: &str, account: &Arc<AcmeAccount>) {
        self.siblings.host.set_context(Some(Arc::new(HostContext {
            user: user.to_string(),
            slug: slug.to_string(),
            base_domain: self.base().to_string(),
            account: Arc::clone(account) as Arc<dyn AccountSigner>,
        })));
    }

    fn refresh_sibling_status(&self) {
        let joined: Vec<SiblingInfo> = self
            .siblings
            .keys
            .joined()
            .into_iter()
            .map(|s| SiblingInfo {
                slug: s.slug,
                display_name: s.display_name,
            })
            .collect();
        self.status.send_modify(|status| status.siblings = joined);
    }

    /// Tell sibling discovery where the directory is and whom to call, while
    /// the secure tunnel is connected and this install has an identity.
    fn publish_discovery(&self) {
        let token = self
            .lock_session()
            .as_ref()
            .map(|session| session.directory_token.clone());
        let (Some(identity), Some(token)) = (self.store.identity(), token) else {
            return;
        };
        let base = self.base().to_string();
        let siblings = self
            .siblings
            .keys
            .joined()
            .into_iter()
            .filter_map(|s| {
                self.siblings
                    .keys
                    .outbound_key(&s.slug)
                    .map(|key| (s.slug, key))
            })
            .collect();
        let origin_base = base.clone();
        let user = identity.user.clone();
        self.discovery.send_replace(Some(Arc::new(SecureDiscovery {
            own_slug: identity.slug,
            directory_url: format!("https://{base}/a2a/{}/agents", identity.user),
            directory_token: Some(token),
            siblings,
            origin_for: Arc::new(move |slug| format!("https://{slug}.{user}.{origin_base}")),
        })));
    }

    fn pending_joins(&self, instances: &[InstanceInfo]) -> Vec<PendingJoinInfo> {
        self.siblings
            .host
            .pending(Utc::now())
            .into_iter()
            .map(|pending| PendingJoinInfo {
                id: pending.approval_id,
                code: pending.code,
                in_relay_list: (!instances.is_empty())
                    .then(|| instances.iter().any(|i| i.slug == pending.slug)),
                slug: pending.slug,
                display_name: pending.display_name,
                expires_at: pending.expires_at.to_rfc3339(),
            })
            .collect()
    }

    /// The user, slug and display name this instance joins as.
    fn joining_as(&self) -> Result<(String, String, String), ActionError> {
        let (user, slug) = if let Some(identity) = self.store.identity() {
            (identity.user, identity.slug)
        } else {
            let session = self.lock_session();
            let session = session.as_ref().ok_or(ActionError::NotConnected)?;
            (session.user.clone(), session.slug.clone())
        };
        let display_name = self
            .status
            .borrow()
            .instances
            .iter()
            .find(|i| i.slug == slug)
            .map_or_else(|| slug.clone(), |i| i.display_name.clone());
        Ok((user, slug, display_name))
    }

    async fn start_join(self: &Arc<Self>, target: &str) -> Result<(), ActionError> {
        let target = target.trim().to_ascii_lowercase();
        if !is_valid_slug(&target) {
            return Err(ActionError::Invalid(
                "An instance name is 1 to 24 lowercase letters, digits and hyphens.".to_string(),
            ));
        }
        let (user, slug, display_name) = self.joining_as()?;
        if target == slug {
            return Err(ActionError::Invalid(
                "This is that instance. Name another instance of yours to join.".to_string(),
            ));
        }
        let account = self.account().await.map_err(|e| {
            ActionError::Failed(format!(
                "Residuum couldn't set up its certificate account: {e:#}"
            ))
        })?;
        {
            let mut running = self
                .join_task
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if running.as_ref().is_some_and(|task| !task.is_finished()) {
                return Err(ActionError::Invalid(
                    "A join is already waiting for approval. Wait for it to finish first."
                        .to_string(),
                ));
            }
            self.status.send_modify(|status| {
                status.join = Some(JoinProgress {
                    instance: target.clone(),
                    state: JoinState::Waiting,
                    code: None,
                    detail: Some(format!("Contacting \"{target}\".")),
                });
            });
            let inner = Arc::clone(self);
            *running = Some(crate::util::spawn_monitored("sibling-join", async move {
                inner
                    .run_join(account, user, slug, display_name, target)
                    .await;
            }));
        }
        Ok(())
    }

    async fn run_join(
        &self,
        account: Arc<AcmeAccount>,
        user: String,
        slug: String,
        display_name: String,
        target: String,
    ) {
        let status = Arc::clone(&self.status);
        let progress = move |progress: JoinProgress| {
            status.send_modify(|s| s.join = Some(progress));
        };
        let result = self
            .siblings
            .join(
                &Joiner {
                    signer: account.as_ref(),
                    pins: &self.pin_client,
                    store: &self.store,
                    user: &user,
                    own_slug: &slug,
                    display_name: &display_name,
                    base_domain: self.base(),
                    target_slug: &target,
                },
                &progress,
            )
            .await;
        match result {
            Ok(()) => {
                tracing::info!(sibling = %target, "joined a sibling instance");
                self.refresh_sibling_status();
                self.publish_discovery();
                self.kick.notify_one();
            }
            Err(failure) => {
                tracing::warn!(sibling = %target, error = %failure, "joining a sibling instance did not complete");
            }
        }
    }

    async fn approve_join(&self, id: &str) -> Result<(), ActionError> {
        let identity = self.store.identity().ok_or_else(|| {
            ActionError::Failed(
                "This instance isn't set up for remote access yet, so it can't approve others."
                    .to_string(),
            )
        })?;
        let account = self.account().await.map_err(|e| {
            ActionError::Failed(format!(
                "Residuum couldn't set up its certificate account: {e:#}"
            ))
        })?;
        let request = self
            .siblings
            .approve(
                &Approver {
                    signer: account.as_ref(),
                    pins: &self.pin_client,
                    store: &self.store,
                    user: &identity.user,
                },
                id,
            )
            .await
            .map_err(|e| match e {
                ApproveError::Unknown => ActionError::Invalid(e.to_string()),
                ApproveError::Pins(_) | ApproveError::Storage(_) => {
                    ActionError::Failed(e.to_string())
                }
            })?;
        tracing::info!(sibling = %request.slug, account = %request.account_uri, "approved a sibling join");
        self.refresh_sibling_status();
        self.publish_discovery();
        self.refresh_pins().await;
        Ok(())
    }

    fn deny_join(&self, id: &str) -> Result<(), ActionError> {
        if self.siblings.host.deny(id, Utc::now()) {
            Ok(())
        } else {
            Err(ActionError::Invalid(ApproveError::Unknown.to_string()))
        }
    }

    async fn remove_pin(&self, account_uri: &str) -> Result<(), ActionError> {
        let removable = self
            .status
            .borrow()
            .pins
            .iter()
            .find(|pin| pin.account_uri == account_uri)
            .map(|pin| pin.removable);
        match removable {
            None => {
                return Err(ActionError::Invalid(
                    "That certificate account isn't pinned any more.".to_string(),
                ));
            }
            Some(false) => {
                return Err(ActionError::Invalid(
                    "Only accounts of instances that no longer exist in Residuum Cloud can be removed here."
                        .to_string(),
                ));
            }
            Some(true) => {}
        }
        let identity = self.store.identity().ok_or_else(|| {
            ActionError::Failed("This instance isn't set up for remote access yet.".to_string())
        })?;
        let account = self.account().await.map_err(|e| {
            ActionError::Failed(format!(
                "Residuum couldn't set up its certificate account: {e:#}"
            ))
        })?;
        match self
            .pin_client
            .remove(account.as_ref(), &identity.user, account_uri)
            .await
        {
            Ok(pins) => {
                if let Some(slug) = self.siblings.keys.slug_for_account(account_uri) {
                    self.siblings.keys.remove(&slug).await.map_err(|e| {
                        ActionError::Failed(format!("Residuum couldn't forget the sibling: {e:#}"))
                    })?;
                    self.refresh_sibling_status();
                    self.publish_discovery();
                }
                if let Err(e) = self.store.forget_known(account_uri).await {
                    tracing::warn!(error = %format!("{e:#}"), "couldn't forget a removed certificate account");
                }
                tracing::info!(account = %account_uri, "removed a pin for an instance that no longer exists");
                self.record_pins(&pins).await;
                Ok(())
            }
            Err(PinError::Rejected { status: 409, .. }) => Err(ActionError::Failed(
                "The pin service wouldn't remove it: an address always needs at least one pinned account."
                    .to_string(),
            )),
            Err(e) => Err(ActionError::Failed(format!("The pin wasn't removed: {e}"))),
        }
    }

    fn activate_instance(&self, slug: &str) -> Result<(), ActionError> {
        if !is_valid_slug(slug) {
            return Err(ActionError::Invalid(
                "That isn't a valid instance name.".to_string(),
            ));
        }
        if !self
            .status
            .borrow()
            .instances
            .iter()
            .any(|i| i.slug == slug)
        {
            return Err(ActionError::Invalid(
                "Residuum Cloud doesn't list an instance with that name.".to_string(),
            ));
        }
        let session = self.lock_session();
        let session = session.as_ref().ok_or(ActionError::NotConnected)?;
        session.link.activate_instance(slug);
        Ok(())
    }

    /// Keep the relay's list of instances for the switcher, dropping any
    /// entry whose slug isn't one the relay could have issued.
    fn set_instances(&self, instances: Vec<InstanceSummary>) {
        let kept: Vec<InstanceInfo> = instances
            .into_iter()
            .filter(|i| {
                let ok = is_valid_slug(&i.slug);
                if !ok {
                    tracing::warn!("the relay listed an instance with an invalid name; ignored");
                }
                ok
            })
            .map(|i| InstanceInfo {
                display_name: clean_display_name(&i.display_name, &i.slug),
                slug: i.slug,
                active: i.active,
                connected: i.connected,
            })
            .collect();
        self.status.send_modify(|status| {
            for pin in &mut status.pins {
                pin.removable = is_removable(pin.own, &pin.slug, &kept);
            }
            status.instances = kept;
        });
    }
}

/// A relay-supplied name as plain text of reasonable length; the slug stands
/// in for an empty one.
fn clean_display_name(name: &str, slug: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_INSTANCE_NAME)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        slug.to_string()
    } else {
        trimmed.to_string()
    }
}

fn retry_delay(failures: u32) -> Duration {
    let secs = 15_u64.saturating_mul(1_u64 << failures.min(6));
    Duration::from_secs(secs).min(MAX_RETRY)
}

#[async_trait]
impl SessionHandler for RemoteAccess {
    async fn on_connected(&self, connected: &ConnectedInfo, link: RelayLink) -> Verdict {
        let inner = &self.inner;
        let stored = inner.store.identity();
        if let Err(e) = verify_announcement(
            stored.as_ref(),
            inner.base(),
            &connected.user,
            &connected.instance,
            &connected.hosts,
        ) {
            tracing::error!(error = %e, "refusing the relay's tunnel: its identity doesn't match this install");
            let detail = match &e {
                IdentityError::Mismatch { .. } => format!(
                    "Residuum Cloud announced a different identity than this install was set up with, so the tunnel was refused. {e}. If you changed accounts on purpose, remove this install's remote access state in the hub folder."
                ),
                IdentityError::HostsDiffer { .. } | IdentityError::InvalidName => format!(
                    "Residuum Cloud announced addresses this install doesn't accept, so the tunnel was refused: {e}."
                ),
            };
            inner.set_state(RemoteAccessState::Refused, Some(detail.clone()));
            (inner.notify)(detail);
            return Verdict::Refuse(e.to_string());
        }
        let (ui_origin, workbench_origin, instance_origin) =
            stored.as_ref().map_or((None, None, None), |identity| {
                let hosts = Hostnames::derive(&identity.user, &identity.slug, inner.base());
                (
                    Some(format!("https://{}", hosts.ui)),
                    Some(format!("https://{}", hosts.workbench)),
                    Some(format!("https://{}", hosts.instance)),
                )
            });
        inner.set_state(RemoteAccessState::Connecting, None);
        Arc::clone(inner).start_session(
            link,
            connected.user.clone(),
            connected.instance.clone(),
            connected.a2a_token.clone(),
        );
        Verdict::Accept {
            user: connected.user.clone(),
            instance: connected.instance.clone(),
            ui_origin,
            workbench_origin,
            instance_origin,
        }
    }

    fn on_stream(&self, stream: IncomingStream) {
        let engine = Arc::clone(&self.inner.engine);
        crate::util::spawn_monitored("remote-access-stream", async move {
            engine.serve(stream.host, stream.peer_ip, stream.io).await;
        });
    }

    fn on_instances(&self, instances: Vec<InstanceSummary>) {
        self.inner.set_instances(instances);
    }

    fn on_disconnected(&self) {
        let inner = &self.inner;
        if let Some(session) = inner.lock_session().take() {
            session.task.abort();
        }
        inner.siblings.host.set_context(None);
        inner.discovery.send_replace(None);
        inner.status.send_modify(|status| status.instances.clear());
        let refused = inner.status.borrow().state == RemoteAccessState::Refused;
        if !refused {
            inner.set_state(RemoteAccessState::Connecting, None);
        }
    }

    fn on_relay_unsupported(&self) {
        self.inner.set_state(
            RemoteAccessState::Error,
            Some("The relay doesn't offer the secure tunnel, so remote access is down until the relay supports it again. Residuum keeps trying.".to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(slug: &str) -> InstanceInfo {
        InstanceInfo {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            active: false,
            connected: true,
        }
    }

    #[test]
    fn only_pins_of_instances_the_relay_no_longer_lists_are_removable() {
        let listed = [instance("laptop"), instance("desktop")];
        assert!(is_removable(false, "old-box", &listed));
        assert!(!is_removable(false, "desktop", &listed));
        assert!(
            !is_removable(true, "old-box", &listed),
            "never the own account"
        );
        assert!(
            !is_removable(false, "old-box", &[]),
            "an unknown list removes nothing"
        );
    }

    #[test]
    fn relay_display_names_become_short_plain_text() {
        assert_eq!(
            clean_display_name("  Desk\u{1b}[2J\n top ", "d"),
            "Desk[2J top"
        );
        assert_eq!(clean_display_name(" \n", "desk"), "desk");
        assert_eq!(
            clean_display_name(&"x".repeat(200), "d").chars().count(),
            64
        );
        // Markup is text for the UI to escape, not something to strip here.
        assert_eq!(clean_display_name("<b>x</b>", "d"), "<b>x</b>");
    }

    #[test]
    fn retries_back_off_to_a_cap() {
        assert_eq!(retry_delay(1), Duration::from_secs(30));
        assert_eq!(retry_delay(3), Duration::from_secs(120));
        assert_eq!(retry_delay(30), MAX_RETRY);
    }
}
