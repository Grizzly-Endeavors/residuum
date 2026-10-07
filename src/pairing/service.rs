//! [`DevicePairing`]: the pairing state behind a lock, saved after every
//! change that matters, with the real clock.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use tokio::sync::watch;

use super::Surface;
use super::cookies;
use super::error::PairingError;
use super::remote::{RemoteContext, normalize_origin};
use super::secrets;
use super::state::{
    AuthHit, HANDOFF_TTL_SECS, Issued, PAIR_TOKEN_TTL_SECS, PendingView, PollOutcome,
    REQUEST_TTL_SECS, State, clean_device_name,
};
use super::store::{self, Identity, Persisted};
use super::types::{DeviceInfo, PendingPairingInfo};
use crate::tunnel::TunnelStatus;

/// A first-device pairing token, ready to put in a link.
#[derive(Debug, Clone)]
pub(crate) struct MintedPairToken {
    pub(crate) token: String,
    pub(crate) ui_origin: String,
    pub(crate) expires_in_secs: u64,
    /// The recovery codes, when this call generated them.
    pub(crate) new_recovery_codes: Option<Vec<String>>,
}

/// A pairing request just created.
#[derive(Debug, Clone)]
pub(crate) struct CreatedRequest {
    pub(crate) request_id: String,
    pub(crate) code: String,
    pub(crate) expires_in_secs: u64,
}

struct Inner {
    path: PathBuf,
    /// Serializes saves, so an older snapshot never overwrites a newer one.
    write_lock: tokio::sync::Mutex<()>,
    state: Mutex<State>,
    /// Seconds added to the clock. Zero outside tests, which move time with it.
    skew_secs: std::sync::atomic::AtomicI64,
}

/// Device pairing for this install: who may reach it remotely.
///
/// Cheap to clone; every clone shares the state.
#[derive(Clone)]
pub struct DevicePairing {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DevicePairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevicePairing").finish_non_exhaustive()
    }
}

impl DevicePairing {
    /// Open the pairing state stored in `hub_dir`, starting empty when there
    /// is none.
    #[must_use]
    pub fn open(hub_dir: &Path) -> Self {
        let path = crate::config::HubPaths::new(hub_dir).remote_access_json();
        let persisted = store::load(&path);
        Self {
            inner: Arc::new(Inner {
                path,
                write_lock: tokio::sync::Mutex::new(()),
                state: Mutex::new(State::new(persisted)),
                skew_secs: std::sync::atomic::AtomicI64::new(0),
            }),
        }
    }

    /// Keep the stored identity in step with what Residuum Cloud announces on
    /// each connect. Runs until the status channel closes.
    pub fn track_identity(&self, mut status: watch::Receiver<TunnelStatus>) {
        let pairing = self.clone();
        crate::util::spawn_monitored("pairing-identity", async move {
            loop {
                let announced = match &*status.borrow_and_update() {
                    TunnelStatus::Connected {
                        origin,
                        workbench_origin,
                        instance,
                        ..
                    } => Some(Identity {
                        slug: instance.clone(),
                        ui_origin: origin.clone(),
                        workbench_origin: workbench_origin.clone(),
                    }),
                    TunnelStatus::Disconnected | TunnelStatus::Connecting => None,
                };
                if let Some(announced) = announced
                    && let Err(e) = pairing.update_identity(announced).await
                {
                    tracing::warn!(error = %e, "couldn't save the address Residuum Cloud announced; pairing links may not work until it is announced again");
                }
                if status.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    fn now(&self) -> DateTime<Utc> {
        let skew = self
            .inner
            .skew_secs
            .load(std::sync::atomic::Ordering::Relaxed);
        Utc::now() + chrono::Duration::seconds(skew)
    }

    /// Move this instance's clock forward, for tests of expiry.
    #[cfg(test)]
    pub(crate) fn advance_clock(&self, by: chrono::Duration) {
        self.inner
            .skew_secs
            .fetch_add(by.num_seconds(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Pair a device without going through a link, for tests of other systems
    /// that sit behind the gate. Returns the `Cookie` header values that
    /// authenticate it on the UI host and on the workbench host.
    #[cfg(test)]
    pub(crate) async fn pair_device_for_tests(&self, name: &str) -> (String, String) {
        let now = self.now();
        let (ui, workbench) = {
            let mut state = self.lock();
            let ui = state.add_device(name, now).unwrap();
            let workbench = state
                .add_workbench_credential(&ui.device_id, None, now)
                .unwrap();
            (ui, workbench)
        };
        self.save().await.unwrap();
        let cookie_name = self.cookie_name();
        (
            format!("{cookie_name}={}", ui.secret),
            format!("{cookie_name}={}", workbench.secret),
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn save(&self) -> Result<(), PairingError> {
        let _guard = self.inner.write_lock.lock().await;
        let snapshot: Persisted = {
            let mut state = self.lock();
            state.mark_saved();
            state.persisted.clone()
        };
        store::save(&self.inner.path, &snapshot).await
    }

    // ── Identity ─────────────────────────────────────────────────────

    /// What Residuum Cloud last announced.
    #[must_use]
    pub(crate) fn identity(&self) -> Identity {
        self.lock().persisted.identity.clone()
    }

    /// Record what the relay announced. A field the relay left out keeps its
    /// stored value; one it sent that isn't well-formed is ignored.
    ///
    /// # Errors
    /// Returns the storage error if the change couldn't be saved.
    pub(crate) async fn update_identity(&self, announced: Identity) -> Result<(), PairingError> {
        let changed = {
            let mut state = self.lock();
            let current = &mut state.persisted.identity;
            let before = current.clone();
            if let Some(slug) = announced.slug.filter(|s| cookies::is_valid_slug(s)) {
                current.slug = Some(slug);
            }
            if let Some(origin) = announced.ui_origin.as_deref().and_then(normalize_origin) {
                current.ui_origin = Some(origin);
            }
            if let Some(origin) = announced
                .workbench_origin
                .as_deref()
                .and_then(normalize_origin)
            {
                current.workbench_origin = Some(origin);
            }
            *current != before
        };
        if changed { self.save().await } else { Ok(()) }
    }

    /// The name of the device cookie on both hosts.
    #[must_use]
    pub(crate) fn cookie_name(&self) -> String {
        cookies::cookie_name(self.lock().persisted.identity.slug.as_deref())
    }

    /// The origin a browser reaches `surface` at, as announced.
    #[must_use]
    pub(crate) fn origin_of(&self, surface: Surface) -> Option<String> {
        let identity = self.identity();
        match surface {
            Surface::Ui => identity.ui_origin,
            Surface::Workbench => identity.workbench_origin,
        }
    }

    // ── Authentication ───────────────────────────────────────────────

    /// The device whose credential `cookie_header` carries for `surface`.
    pub(crate) async fn authenticate(
        &self,
        surface: Surface,
        cookie_header: Option<&str>,
    ) -> Option<(AuthHit, String)> {
        let name = self.cookie_name();
        let secret = cookies::find_cookie(cookie_header?, &name)?;
        let now = self.now();
        let (hit, flush) = {
            let mut state = self.lock();
            let hit = state.authenticate(surface, secret, now)?;
            (hit, state.flush_due(now))
        };
        if flush && let Err(e) = self.save().await {
            tracing::warn!(error = %e, "couldn't save when a paired device was last seen");
        }
        Some((hit, secret.to_string()))
    }

    /// The `Set-Cookie` value that stores `secret` as this install's device cookie.
    #[must_use]
    pub(crate) fn set_cookie_for(&self, secret: &str) -> String {
        cookies::set_cookie_value(&self.cookie_name(), secret)
    }

    /// The value of this install's device cookie in `cookie_header`, if any.
    #[must_use]
    pub(crate) fn cookie_in(&self, cookie_header: Option<&str>) -> Option<String> {
        cookies::find_cookie(cookie_header?, &self.cookie_name()).map(str::to_string)
    }

    // ── Local bootstrap ──────────────────────────────────────────────

    /// Mint the single-use token for the first device. Also creates the
    /// recovery codes the first time, and returns them for showing once.
    ///
    /// # Errors
    /// `NotReady` when Residuum Cloud has not yet announced this install's
    /// address, or a storage or random-source error.
    pub(crate) async fn mint_pair_token(&self) -> Result<MintedPairToken, PairingError> {
        let now = self.now();
        let token = secrets::secret_256()?;
        let (ui_origin, new_recovery_codes) = {
            let mut state = self.lock();
            let ui_origin = state.persisted.identity.ui_origin.clone().ok_or_else(|| {
                PairingError::NotReady(
                    "Residuum Cloud hasn't told this install its web address yet. Connect to Residuum Cloud first (Settings, All agents, Residuum Cloud), then try again."
                        .to_string(),
                )
            })?;
            let codes = if state.persisted.recovery.generated_at.is_none() {
                Some(state.replace_recovery_codes(now)?)
            } else {
                None
            };
            state.add_pair_token(secrets::hash(&token), now);
            (ui_origin, codes)
        };
        self.save().await?;
        Ok(MintedPairToken {
            token,
            ui_origin,
            expires_in_secs: u64::try_from(PAIR_TOKEN_TTL_SECS).unwrap_or(600),
            new_recovery_codes,
        })
    }

    /// Replace the recovery codes with ten new ones. The old ones stop working.
    ///
    /// # Errors
    /// A storage or random-source error.
    pub(crate) async fn regenerate_recovery_codes(&self) -> Result<Vec<String>, PairingError> {
        let now = self.now();
        let codes = self.lock().replace_recovery_codes(now)?;
        self.save().await?;
        Ok(codes)
    }

    // ── Pairing flows ────────────────────────────────────────────────

    fn limit(&self, remote: Option<&RemoteContext>) -> Result<(), PairingError> {
        let Some(remote) = remote else { return Ok(()) };
        let now = self.now();
        self.lock().check_limit(remote.peer_ip.as_deref(), now)
    }

    /// Pair a device that presents a first-device token.
    ///
    /// # Errors
    /// `RateLimited`, `Rejected` for a bad, spent or expired token, or storage.
    pub(crate) async fn redeem_token(
        &self,
        remote: Option<&RemoteContext>,
        token: &str,
        device_name: &str,
    ) -> Result<Issued, PairingError> {
        self.limit(remote)?;
        let now = self.now();
        let issued = {
            let mut state = self.lock();
            state.take_pair_token(&secrets::hash(token), now)?;
            state.add_device(device_name, now)?
        };
        self.save().await?;
        tracing::info!(device = %issued.device_name, device_id = %issued.device_id, "paired a device with a pairing link");
        Ok(issued)
    }

    /// Pair a device that presents a recovery code.
    ///
    /// # Errors
    /// `RateLimited`, `Rejected` for a wrong or used code, or storage.
    pub(crate) async fn redeem_recovery(
        &self,
        remote: Option<&RemoteContext>,
        code: &str,
        device_name: &str,
    ) -> Result<Issued, PairingError> {
        self.limit(remote)?;
        let now = self.now();
        let (issued, remaining) = {
            let mut state = self.lock();
            state.take_recovery_code(code)?;
            let issued = state.add_device(device_name, now)?;
            (issued, state.recovery_codes_remaining())
        };
        self.save().await?;
        tracing::warn!(device = %issued.device_name, device_id = %issued.device_id, remaining, "paired a device with a recovery code");
        Ok(issued)
    }

    /// Open a pairing request for a browser that has no credential.
    ///
    /// # Errors
    /// `RateLimited` past the limits or ten waiting requests, or random-source.
    pub(crate) fn create_request(
        &self,
        remote: Option<&RemoteContext>,
        device_name: &str,
    ) -> Result<CreatedRequest, PairingError> {
        self.limit(remote)?;
        let now = self.now();
        let request_id = secrets::secret_128()?;
        let name = clean_device_name(device_name);
        let (_, code) = self
            .lock()
            .add_request(secrets::hash(&request_id), name.clone(), now)?;
        tracing::info!(device = %name, "a device asked to be paired");
        Ok(CreatedRequest {
            request_id,
            code,
            expires_in_secs: u64::try_from(REQUEST_TTL_SECS).unwrap_or(600),
        })
    }

    /// Check on the request whose secret id is `request_id`. An approved
    /// request issues its credential here, once.
    ///
    /// # Errors
    /// A storage or random-source error.
    pub(crate) async fn poll_request(&self, request_id: &str) -> Result<PollOutcome, PairingError> {
        let now = self.now();
        let outcome = self.lock().poll(&secrets::hash(request_id), now)?;
        if matches!(outcome, PollOutcome::Approved(_)) {
            self.save().await?;
        }
        Ok(outcome)
    }

    /// The pairing requests waiting for a decision.
    #[must_use]
    pub(crate) fn pending(&self) -> Vec<PendingPairingInfo> {
        let now = self.now();
        let views: Vec<PendingView> = self.lock().pending(now);
        views
            .into_iter()
            .map(|view| PendingPairingInfo {
                id: view.id,
                code: view.code,
                device_name: view.device_name,
                created_at: view.created_at,
                expires_at: view.expires_at,
            })
            .collect()
    }

    /// Approve or refuse a waiting request.
    ///
    /// # Errors
    /// `Rejected` when the request is gone.
    pub(crate) fn decide(&self, public_id: &str, approve: bool) -> Result<(), PairingError> {
        let now = self.now();
        self.lock().decide(public_id, approve, now)?;
        tracing::info!(approved = approve, "answered a pairing request");
        Ok(())
    }

    // ── Workbench handoff ────────────────────────────────────────────

    /// Mint the single-use token a paired UI-host browser carries to the
    /// workbench host.
    ///
    /// # Errors
    /// A random-source error.
    pub(crate) fn mint_handoff(&self, device_id: &str) -> Result<(String, u64), PairingError> {
        let now = self.now();
        let token = secrets::secret_256()?;
        self.lock()
            .add_handoff(secrets::hash(&token), device_id.to_string(), now);
        Ok((token, u64::try_from(HANDOFF_TTL_SECS).unwrap_or(60)))
    }

    /// Spend a handoff token on the workbench host, issuing (or confirming)
    /// that browser's workbench credential.
    ///
    /// # Errors
    /// `RateLimited`, `Rejected` for a bad, spent or expired token, or storage.
    pub(crate) async fn redeem_handoff(
        &self,
        remote: Option<&RemoteContext>,
        token: &str,
        held_credential: Option<&str>,
    ) -> Result<Issued, PairingError> {
        self.limit(remote)?;
        let now = self.now();
        let issued = {
            let mut state = self.lock();
            let device_id = state.take_handoff(&secrets::hash(token), now)?;
            state.add_workbench_credential(&device_id, held_credential, now)?
        };
        self.save().await?;
        Ok(issued)
    }

    // ── Listing and revoking ─────────────────────────────────────────

    /// Every paired device, oldest first. `current` marks the asking browser's.
    #[must_use]
    pub(crate) fn devices(&self, current: Option<&str>) -> Vec<DeviceInfo> {
        self.lock()
            .persisted
            .devices
            .iter()
            .map(|d| DeviceInfo {
                id: d.id.clone(),
                name: d.name.clone(),
                created_at: d.created_at,
                last_seen: d.last_seen,
                current: current == Some(d.id.as_str()),
            })
            .collect()
    }

    /// Unused recovery codes left.
    #[must_use]
    pub(crate) fn recovery_codes_remaining(&self) -> usize {
        self.lock().recovery_codes_remaining()
    }

    /// Stop trusting a device. It is refused on its next request.
    ///
    /// # Errors
    /// `Rejected` when there is no such device, or a storage error.
    pub(crate) async fn revoke(&self, device_id: &str) -> Result<(), PairingError> {
        if !self.lock().revoke(device_id) {
            return Err(PairingError::Rejected(
                "There's no paired device with that id. It may already have been removed."
                    .to_string(),
            ));
        }
        self.save().await?;
        tracing::warn!(device_id, "revoked a paired device");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn open() -> (tempfile::TempDir, DevicePairing) {
        let dir = tempfile::tempdir().unwrap();
        let pairing = DevicePairing::open(dir.path());
        (dir, pairing)
    }

    async fn with_identity(pairing: &DevicePairing) {
        pairing
            .update_identity(Identity {
                slug: Some("laptop".to_string()),
                ui_origin: Some("https://bear.agent-residuum.com".to_string()),
                workbench_origin: Some("https://bear.workbench.agent-residuum.com".to_string()),
            })
            .await
            .unwrap();
    }

    fn cookie_header(pairing: &DevicePairing, issued: &Issued) -> String {
        format!("{}={}", pairing.cookie_name(), issued.secret)
    }

    #[tokio::test]
    async fn the_first_device_pairs_with_a_link_token_and_survives_a_restart() {
        let (dir, pairing) = open();
        with_identity(&pairing).await;
        let minted = pairing.mint_pair_token().await.unwrap();
        assert_eq!(minted.ui_origin, "https://bear.agent-residuum.com");
        assert_eq!(
            minted.new_recovery_codes.as_ref().map(Vec::len),
            Some(10),
            "the first token also creates the ten recovery codes"
        );
        let issued = pairing
            .redeem_token(None, &minted.token, "Laptop")
            .await
            .unwrap();
        assert!(
            pairing
                .authenticate(Surface::Ui, Some(&cookie_header(&pairing, &issued)))
                .await
                .is_some()
        );

        let reopened = DevicePairing::open(dir.path());
        assert!(
            reopened
                .authenticate(Surface::Ui, Some(&cookie_header(&reopened, &issued)))
                .await
                .is_some(),
            "a paired device stays paired across a restart"
        );
        let file = std::fs::read_to_string(dir.path().join("remote-access.json")).unwrap();
        assert!(
            !file.contains(&issued.secret),
            "the file must hold only hashes, never the credential"
        );
    }

    #[tokio::test]
    async fn the_second_token_does_not_recreate_recovery_codes() {
        let (_dir, pairing) = open();
        with_identity(&pairing).await;
        assert!(
            pairing
                .mint_pair_token()
                .await
                .unwrap()
                .new_recovery_codes
                .is_some()
        );
        assert!(
            pairing
                .mint_pair_token()
                .await
                .unwrap()
                .new_recovery_codes
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_pairing_link_needs_the_announced_address() {
        let (_dir, pairing) = open();
        let refused = pairing.mint_pair_token().await.unwrap_err();
        assert!(matches!(refused, PairingError::NotReady(_)), "{refused:?}");
    }

    #[tokio::test]
    async fn a_token_cannot_be_reused_and_expires() {
        let (_dir, pairing) = open();
        with_identity(&pairing).await;
        let minted = pairing.mint_pair_token().await.unwrap();
        pairing
            .redeem_token(None, &minted.token, "A")
            .await
            .unwrap();
        assert!(
            pairing
                .redeem_token(None, &minted.token, "B")
                .await
                .is_err(),
            "a token is single-use"
        );

        let stale = pairing.mint_pair_token().await.unwrap();
        pairing.advance_clock(Duration::seconds(PAIR_TOKEN_TTL_SECS + 1));
        assert!(
            pairing.redeem_token(None, &stale.token, "C").await.is_err(),
            "a token expires after ten minutes"
        );
    }

    #[tokio::test]
    async fn an_additional_device_is_approved_from_a_paired_one() {
        let (_dir, pairing) = open();
        let created = pairing.create_request(None, "Phone").unwrap();
        assert_eq!(created.code.len(), 6);
        assert!(matches!(
            pairing.poll_request(&created.request_id).await.unwrap(),
            PollOutcome::Pending
        ));
        let pending = pairing.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.first().unwrap().code, created.code);
        pairing.decide(&pending.first().unwrap().id, true).unwrap();
        let PollOutcome::Approved(issued) =
            pairing.poll_request(&created.request_id).await.unwrap()
        else {
            panic!("the approved request should issue a credential");
        };
        assert!(
            pairing
                .authenticate(Surface::Ui, Some(&cookie_header(&pairing, &issued)))
                .await
                .is_some()
        );
        assert!(pairing.pending().is_empty());
    }

    #[tokio::test]
    async fn an_unapproved_request_expires_after_ten_minutes() {
        let (_dir, pairing) = open();
        let created = pairing.create_request(None, "Phone").unwrap();
        pairing.advance_clock(Duration::seconds(REQUEST_TTL_SECS + 1));
        assert!(matches!(
            pairing.poll_request(&created.request_id).await.unwrap(),
            PollOutcome::Expired
        ));
    }

    #[tokio::test]
    async fn more_than_ten_waiting_requests_are_refused() {
        let (_dir, pairing) = open();
        for _ in 0..10 {
            pairing.create_request(None, "Phone").unwrap();
        }
        assert!(pairing.create_request(None, "Phone").is_err());
    }

    #[tokio::test]
    async fn remote_request_creation_is_rate_limited_per_address() {
        let (_dir, pairing) = open();
        let remote = RemoteContext {
            peer_ip: Some("203.0.113.5".to_string()),
            origin: None,
        };
        let mut refused = None;
        for _ in 0..12 {
            // Approve each so the cap on waiting requests isn't what refuses.
            match pairing.create_request(Some(&remote), "Phone") {
                Ok(created) => {
                    let pending = pairing.pending();
                    for p in pending {
                        pairing.decide(&p.id, false).ok();
                    }
                    pairing.poll_request(&created.request_id).await.unwrap();
                }
                Err(e) => refused = Some(e),
            }
        }
        assert!(
            matches!(refused, Some(PairingError::RateLimited(_))),
            "the eleventh request in a minute from one address must be limited: {refused:?}"
        );
    }

    #[tokio::test]
    async fn recovery_code_entry_is_rate_limited_too() {
        let (_dir, pairing) = open();
        let remote = RemoteContext {
            peer_ip: Some("203.0.113.5".to_string()),
            origin: None,
        };
        let mut limited = false;
        for _ in 0..11 {
            if let Err(PairingError::RateLimited(_)) = pairing
                .redeem_recovery(Some(&remote), "AAAAAAAAAAAAAAAA", "x")
                .await
            {
                limited = true;
            }
        }
        assert!(limited, "ten guesses a minute is the most one address gets");
    }

    #[tokio::test]
    async fn a_recovery_code_pairs_a_device_once() {
        let (_dir, pairing) = open();
        with_identity(&pairing).await;
        let codes = pairing
            .mint_pair_token()
            .await
            .unwrap()
            .new_recovery_codes
            .unwrap();
        let code = codes.first().unwrap();
        let issued = pairing.redeem_recovery(None, code, "Tablet").await.unwrap();
        assert_eq!(issued.device_name, "Tablet");
        assert!(pairing.redeem_recovery(None, code, "Again").await.is_err());
        assert_eq!(pairing.recovery_codes_remaining(), 9);
    }

    #[tokio::test]
    async fn a_revoked_device_is_refused_and_stays_refused_after_a_restart() {
        let (dir, pairing) = open();
        with_identity(&pairing).await;
        let minted = pairing.mint_pair_token().await.unwrap();
        let issued = pairing
            .redeem_token(None, &minted.token, "Laptop")
            .await
            .unwrap();
        let header = cookie_header(&pairing, &issued);
        pairing.revoke(&issued.device_id).await.unwrap();
        assert!(
            pairing
                .authenticate(Surface::Ui, Some(&header))
                .await
                .is_none()
        );
        let reopened = DevicePairing::open(dir.path());
        assert!(
            reopened
                .authenticate(Surface::Ui, Some(&header))
                .await
                .is_none()
        );
        assert!(pairing.revoke(&issued.device_id).await.is_err());
    }

    #[tokio::test]
    async fn a_workbench_handoff_issues_a_workbench_credential_for_the_same_device() {
        let (_dir, pairing) = open();
        with_identity(&pairing).await;
        let minted = pairing.mint_pair_token().await.unwrap();
        let ui = pairing
            .redeem_token(None, &minted.token, "Laptop")
            .await
            .unwrap();
        let (token, _) = pairing.mint_handoff(&ui.device_id).unwrap();
        let wb = pairing.redeem_handoff(None, &token, None).await.unwrap();
        assert_eq!(wb.device_id, ui.device_id);
        assert!(
            pairing
                .authenticate(Surface::Workbench, Some(&cookie_header(&pairing, &wb)))
                .await
                .is_some()
        );
        assert!(
            pairing.redeem_handoff(None, &token, None).await.is_err(),
            "a handoff token is single-use"
        );
        pairing.revoke(&ui.device_id).await.unwrap();
        assert!(
            pairing
                .authenticate(Surface::Workbench, Some(&cookie_header(&pairing, &wb)))
                .await
                .is_none(),
            "revoking the device ends its workbench access too"
        );
    }

    #[tokio::test]
    async fn the_cookie_name_uses_the_slug_and_falls_back_without_one() {
        let (_dir, pairing) = open();
        assert_eq!(pairing.cookie_name(), "__Host-residuum_device_default");
        with_identity(&pairing).await;
        assert_eq!(pairing.cookie_name(), "__Host-residuum_device_laptop");
    }

    #[tokio::test]
    async fn an_announcement_missing_fields_keeps_what_was_stored() {
        let (_dir, pairing) = open();
        with_identity(&pairing).await;
        pairing
            .update_identity(Identity {
                slug: None,
                ui_origin: Some("javascript:alert(1)".to_string()),
                workbench_origin: None,
            })
            .await
            .unwrap();
        let identity = pairing.identity();
        assert_eq!(identity.slug.as_deref(), Some("laptop"));
        assert_eq!(
            identity.ui_origin.as_deref(),
            Some("https://bear.agent-residuum.com"),
            "a malformed origin is ignored"
        );
    }

    #[tokio::test]
    async fn the_identity_follows_the_tunnel_status() {
        let (dir, pairing) = open();
        let (tx, rx) = watch::channel(TunnelStatus::Disconnected);
        pairing.track_identity(rx);
        tx.send(TunnelStatus::Connected {
            user_id: "bear".to_string(),
            origin: Some("https://bear.agent-residuum.com".to_string()),
            workbench_origin: Some("https://bear.workbench.agent-residuum.com".to_string()),
            instance: Some("laptop".to_string()),
            a2a_token: None,
        })
        .unwrap();
        // The identity is set in memory first and saved after, so wait for the file.
        let mut saved = None;
        for _ in 0..250 {
            saved = DevicePairing::open(dir.path()).identity().slug;
            if saved.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            saved.as_deref(),
            Some("laptop"),
            "the announced identity is saved"
        );
        assert_eq!(pairing.identity().slug.as_deref(), Some("laptop"));
    }
}
