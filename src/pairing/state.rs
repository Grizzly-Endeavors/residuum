//! The pairing state and every rule that changes it, as plain synchronous
//! code that takes the current time as an argument.
//!
//! [`super::DevicePairing`] wraps this in a lock, saves the file after a
//! change, and supplies the clock; keeping the rules here makes expiry and
//! limits testable without waiting.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use super::Surface;
use super::error::PairingError;
use super::limits::{LimitHit, PER_INSTANCE_LIMIT, PER_PEER_LIMIT, RateLimiter};
use super::secrets;
use super::store::{Persisted, StoredDevice};

/// How long a first-device pairing token works.
pub(super) const PAIR_TOKEN_TTL_SECS: i64 = 600;

/// How long a workbench handoff token works.
pub(super) const HANDOFF_TTL_SECS: i64 = 60;

/// How long a pairing request waits for approval.
pub(super) const REQUEST_TTL_SECS: i64 = 600;

/// How many pairing requests can wait at once.
pub(super) const MAX_PENDING_REQUESTS: usize = 10;

/// A device that makes no request for this long is no longer paired. The
/// cookie's `Max-Age` is the same length.
pub(super) const DEVICE_LIFETIME_DAYS: i64 = 400;

/// A device's cookie is issued again when it was last issued this long ago.
const COOKIE_REFRESH_AFTER_HOURS: i64 = 24;

/// A change only `last_seen` or `refreshed_at` is saved after this long.
pub(super) const FLUSH_AFTER_MINUTES: i64 = 10;

/// Workbench credentials kept per device.
const MAX_WORKBENCH_CREDENTIALS: usize = 8;

/// Outstanding tokens of each kind. Older ones are dropped past this.
const MAX_OUTSTANDING_TOKENS: usize = 50;

/// The longest device name kept, in characters.
const MAX_DEVICE_NAME_CHARS: usize = 64;

/// What an unnamed device is called.
const DEFAULT_DEVICE_NAME: &str = "Unnamed device";

/// Recovery codes generated at a time.
pub(super) const RECOVERY_CODE_COUNT: usize = 10;

/// A credential just issued. The secret exists only here and in the cookie.
#[derive(Debug, Clone)]
pub(crate) struct Issued {
    pub(crate) secret: String,
    pub(crate) device_id: String,
    pub(crate) device_name: String,
}

/// A device that presented a valid credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthHit {
    pub(crate) device_id: String,
    pub(crate) device_name: String,
    /// Whether the cookie should be issued again to extend its life.
    pub(crate) refresh: bool,
}

#[derive(Debug)]
struct Handoff {
    device_id: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Waiting,
    Approved,
    Denied,
}

#[derive(Debug)]
struct PendingRequest {
    public_id: String,
    secret_hash: String,
    code: String,
    device_name: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    decision: Decision,
}

/// A pending request as the approving device lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingView {
    pub(super) id: String,
    pub(super) code: String,
    pub(super) device_name: String,
    pub(super) created_at: DateTime<Utc>,
    pub(super) expires_at: DateTime<Utc>,
}

/// What polling a pairing request found.
#[derive(Debug)]
pub(crate) enum PollOutcome {
    Pending,
    Approved(Issued),
    Denied,
    Expired,
}

/// The pairing state: what is persisted, and what only lives in memory.
#[derive(Debug, Default)]
pub(super) struct State {
    pub(super) persisted: Persisted,
    pair_tokens: HashMap<String, DateTime<Utc>>,
    handoffs: HashMap<String, Handoff>,
    requests: Vec<PendingRequest>,
    limiter: RateLimiter,
    /// Since when a change only the clock made (`last_seen`, a cookie refresh)
    /// has been waiting to be saved.
    dirty_since: Option<DateTime<Utc>>,
}

impl State {
    pub(super) fn new(persisted: Persisted) -> Self {
        Self {
            persisted,
            ..Self::default()
        }
    }

    /// Count a remote attempt against the limits, or refuse it in words.
    pub(super) fn check_limit(
        &mut self,
        peer_ip: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), PairingError> {
        self.limiter.check(peer_ip, now).map_err(|hit| match hit {
            LimitHit::Peer => PairingError::RateLimited(format!(
                "Too many pairing attempts from this address. Wait a minute, then try again ({PER_PEER_LIMIT} a minute are allowed)."
            )),
            LimitHit::Instance => PairingError::RateLimited(format!(
                "Residuum is getting too many pairing attempts at once. Wait a minute, then try again ({PER_INSTANCE_LIMIT} a minute are allowed in all)."
            )),
        })
    }

    // ── Tokens ───────────────────────────────────────────────────────

    /// Remember a first-device token by its hash.
    pub(super) fn add_pair_token(&mut self, token_hash: String, now: DateTime<Utc>) {
        self.pair_tokens.retain(|_, expires| *expires > now);
        drop_oldest(&mut self.pair_tokens, MAX_OUTSTANDING_TOKENS, |e| *e);
        self.pair_tokens
            .insert(token_hash, now + Duration::seconds(PAIR_TOKEN_TTL_SECS));
    }

    /// Spend a first-device token. A token works once, and only until it expires.
    pub(super) fn take_pair_token(
        &mut self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<(), PairingError> {
        match self.pair_tokens.remove(token_hash) {
            Some(expires) if expires > now => Ok(()),
            _ => Err(PairingError::Rejected(
                "This pairing link has expired or was already used. Make a new one from Residuum on the machine it runs on."
                    .to_string(),
            )),
        }
    }

    /// Remember a workbench handoff token for `device_id`.
    pub(super) fn add_handoff(
        &mut self,
        token_hash: String,
        device_id: String,
        now: DateTime<Utc>,
    ) {
        self.handoffs.retain(|_, h| h.expires_at > now);
        drop_oldest(&mut self.handoffs, MAX_OUTSTANDING_TOKENS, |h| h.expires_at);
        self.handoffs.insert(
            token_hash,
            Handoff {
                device_id,
                expires_at: now + Duration::seconds(HANDOFF_TTL_SECS),
            },
        );
    }

    /// Spend a handoff token, returning the device it was minted for.
    pub(super) fn take_handoff(
        &mut self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<String, PairingError> {
        match self.handoffs.remove(token_hash) {
            Some(handoff) if handoff.expires_at > now => Ok(handoff.device_id),
            _ => Err(PairingError::Rejected(
                "This link has expired or was already used. Open the workbench again from Residuum."
                    .to_string(),
            )),
        }
    }

    // ── Pairing requests ─────────────────────────────────────────────

    /// Add a request waiting for approval and return its public id and code.
    pub(super) fn add_request(
        &mut self,
        secret_hash: String,
        device_name: String,
        now: DateTime<Utc>,
    ) -> Result<(String, String), PairingError> {
        self.purge_requests(now);
        if self.requests.len() >= MAX_PENDING_REQUESTS {
            return Err(PairingError::RateLimited(format!(
                "{MAX_PENDING_REQUESTS} devices are already waiting to be paired. Approve or refuse them from a paired device, or wait for them to expire."
            )));
        }
        let public_id = secrets::public_id()?;
        let code = secrets::base32_code(secrets::PAIRING_CODE_LEN)?;
        self.requests.push(PendingRequest {
            public_id: public_id.clone(),
            secret_hash,
            code: code.clone(),
            device_name,
            created_at: now,
            expires_at: now + Duration::seconds(REQUEST_TTL_SECS),
            decision: Decision::Waiting,
        });
        Ok((public_id, code))
    }

    /// The requests still waiting for a decision, oldest first.
    pub(super) fn pending(&mut self, now: DateTime<Utc>) -> Vec<PendingView> {
        self.purge_requests(now);
        self.requests
            .iter()
            .filter(|r| r.decision == Decision::Waiting)
            .map(|r| PendingView {
                id: r.public_id.clone(),
                code: r.code.clone(),
                device_name: r.device_name.clone(),
                created_at: r.created_at,
                expires_at: r.expires_at,
            })
            .collect()
    }

    /// Approve or refuse the waiting request `public_id`.
    pub(super) fn decide(
        &mut self,
        public_id: &str,
        approve: bool,
        now: DateTime<Utc>,
    ) -> Result<(), PairingError> {
        self.purge_requests(now);
        let request = self
            .requests
            .iter_mut()
            .find(|r| r.public_id == public_id && r.decision == Decision::Waiting)
            .ok_or_else(|| {
                PairingError::Rejected(
                    "That pairing request is gone. It expired, or someone else already answered it."
                        .to_string(),
                )
            })?;
        request.decision = if approve {
            Decision::Approved
        } else {
            Decision::Denied
        };
        Ok(())
    }

    /// Check on a request by the hash of its secret id. An approved request
    /// is spent here: the credential is issued once, to whoever polls.
    pub(super) fn poll(
        &mut self,
        secret_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<PollOutcome, PairingError> {
        self.purge_requests(now);
        let Some(index) = self
            .requests
            .iter()
            .position(|r| secrets::hashes_equal(&r.secret_hash, secret_hash))
        else {
            return Ok(PollOutcome::Expired);
        };
        let decision = self.requests.get(index).map(|r| r.decision);
        match decision {
            Some(Decision::Waiting) => Ok(PollOutcome::Pending),
            Some(Decision::Denied) => {
                self.requests.remove(index);
                Ok(PollOutcome::Denied)
            }
            Some(Decision::Approved) => {
                let request = self.requests.remove(index);
                let issued = self.add_device(&request.device_name, now)?;
                Ok(PollOutcome::Approved(issued))
            }
            None => Ok(PollOutcome::Expired),
        }
    }

    fn purge_requests(&mut self, now: DateTime<Utc>) {
        self.requests.retain(|r| r.expires_at > now);
    }

    // ── Recovery codes ───────────────────────────────────────────────

    /// Replace the recovery codes with ten new ones and return them in the
    /// form to show: grouped for reading.
    pub(super) fn replace_recovery_codes(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, PairingError> {
        let mut plain = Vec::with_capacity(RECOVERY_CODE_COUNT);
        let mut hashes = Vec::with_capacity(RECOVERY_CODE_COUNT);
        for _ in 0..RECOVERY_CODE_COUNT {
            let code = secrets::base32_code(secrets::RECOVERY_CODE_LEN)?;
            hashes.push(secrets::hash(&code));
            plain.push(secrets::group_recovery_code(&code));
        }
        self.persisted.recovery.hashes = hashes;
        self.persisted.recovery.generated_at = Some(now);
        Ok(plain)
    }

    /// Spend the recovery code the person typed.
    pub(super) fn take_recovery_code(&mut self, typed: &str) -> Result<(), PairingError> {
        let normalized = secrets::normalize_recovery_code(typed);
        let rejected = || {
            PairingError::Rejected(
                "That recovery code isn't right, or it was already used. Check it and try again."
                    .to_string(),
            )
        };
        if normalized.len() != secrets::RECOVERY_CODE_LEN {
            return Err(rejected());
        }
        let wanted = secrets::hash(&normalized);
        // Every stored hash is compared, so how long this takes doesn't say
        // which position matched.
        let mut found = None;
        for (index, stored) in self.persisted.recovery.hashes.iter().enumerate() {
            if secrets::hashes_equal(stored, &wanted) {
                found = Some(index);
            }
        }
        match found {
            Some(index) => {
                self.persisted.recovery.hashes.remove(index);
                Ok(())
            }
            None => Err(rejected()),
        }
    }

    pub(super) fn recovery_codes_remaining(&self) -> usize {
        self.persisted.recovery.hashes.len()
    }

    // ── Devices ──────────────────────────────────────────────────────

    /// Pair a new device on the UI host and issue its credential.
    pub(super) fn add_device(
        &mut self,
        name: &str,
        now: DateTime<Utc>,
    ) -> Result<Issued, PairingError> {
        let secret = secrets::secret_256()?;
        let id = secrets::public_id()?;
        let name = clean_device_name(name);
        self.persisted.devices.push(StoredDevice {
            id: id.clone(),
            name: name.clone(),
            created_at: now,
            last_seen: now,
            refreshed_at: Some(now),
            ui_hash: Some(secrets::hash(&secret)),
            workbench_hashes: Vec::new(),
        });
        Ok(Issued {
            secret,
            device_id: id,
            device_name: name,
        })
    }

    /// Issue `device_id` a credential on the workbench host. A browser that
    /// already holds one (`existing`) keeps it and gets no second.
    pub(super) fn add_workbench_credential(
        &mut self,
        device_id: &str,
        existing: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Issued, PairingError> {
        let device = self
            .persisted
            .devices
            .iter_mut()
            .find(|d| d.id == device_id)
            .ok_or_else(|| {
                PairingError::Rejected(
                    "The device that opened this link is no longer paired. Pair it again from Residuum."
                        .to_string(),
                )
            })?;
        let secret = match existing {
            Some(held) if device.workbench_hashes.contains(&secrets::hash(held)) => {
                held.to_string()
            }
            _ => {
                let secret = secrets::secret_256()?;
                device.workbench_hashes.push(secrets::hash(&secret));
                if device.workbench_hashes.len() > MAX_WORKBENCH_CREDENTIALS {
                    device.workbench_hashes.remove(0);
                }
                secret
            }
        };
        device.last_seen = now;
        device.refreshed_at = Some(now);
        Ok(Issued {
            secret,
            device_id: device.id.clone(),
            device_name: device.name.clone(),
        })
    }

    /// Whether `cookie_secret` is a valid credential for `surface`, noting the
    /// use. `Err` is not possible; an unknown or expired credential is `None`.
    pub(super) fn authenticate(
        &mut self,
        surface: Surface,
        cookie_secret: &str,
        now: DateTime<Utc>,
    ) -> Option<AuthHit> {
        let wanted = secrets::hash(cookie_secret);
        let lifetime = Duration::days(DEVICE_LIFETIME_DAYS);
        let device = self.persisted.devices.iter_mut().find(|d| match surface {
            Surface::Ui => d
                .ui_hash
                .as_deref()
                .is_some_and(|h| secrets::hashes_equal(h, &wanted)),
            Surface::Workbench => d
                .workbench_hashes
                .iter()
                .any(|h| secrets::hashes_equal(h, &wanted)),
        })?;
        if now - device.last_seen > lifetime {
            return None;
        }
        let refresh = now - device.refreshed_at.unwrap_or(device.created_at)
            > Duration::hours(COOKIE_REFRESH_AFTER_HOURS);
        let moved = now - device.last_seen > Duration::minutes(1);
        if moved {
            device.last_seen = now;
        }
        if refresh {
            device.refreshed_at = Some(now);
        }
        let hit = AuthHit {
            device_id: device.id.clone(),
            device_name: device.name.clone(),
            refresh,
        };
        if (moved || refresh) && self.dirty_since.is_none() {
            self.dirty_since = Some(now);
        }
        Some(hit)
    }

    /// Whether a change only the clock made has waited long enough to save.
    pub(super) fn flush_due(&self, now: DateTime<Utc>) -> bool {
        self.dirty_since
            .is_some_and(|since| now - since >= Duration::minutes(FLUSH_AFTER_MINUTES))
    }

    /// Note that the file now holds everything.
    pub(super) fn mark_saved(&mut self) {
        self.dirty_since = None;
    }

    /// Remove a device, and with it every credential it holds. Returns whether
    /// there was one.
    pub(super) fn revoke(&mut self, device_id: &str) -> bool {
        let before = self.persisted.devices.len();
        self.persisted.devices.retain(|d| d.id != device_id);
        // A handoff minted by the device can't outlive it.
        self.handoffs.retain(|_, h| h.device_id != device_id);
        self.persisted.devices.len() != before
    }
}

/// Keep at most `max - 1` entries, dropping the ones that expire soonest, to
/// make room for one more.
fn drop_oldest<V>(map: &mut HashMap<String, V>, max: usize, expiry: impl Fn(&V) -> DateTime<Utc>) {
    while map.len() >= max {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, v)| expiry(v))
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        map.remove(&oldest);
    }
}

/// A device name worth listing: trimmed, without control characters, no longer
/// than [`MAX_DEVICE_NAME_CHARS`], and never empty.
pub(super) fn clean_device_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        DEFAULT_DEVICE_NAME.to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn device_names_are_cleaned() {
        assert_eq!(clean_device_name("  Phone \n"), "Phone");
        assert_eq!(clean_device_name(""), DEFAULT_DEVICE_NAME);
        assert_eq!(clean_device_name("\u{7}\u{8}"), DEFAULT_DEVICE_NAME);
        assert_eq!(
            clean_device_name(&"x".repeat(200)).chars().count(),
            MAX_DEVICE_NAME_CHARS
        );
    }

    #[test]
    fn a_pair_token_works_once() {
        let mut state = State::default();
        let t = now();
        state.add_pair_token("h".to_string(), t);
        assert!(state.take_pair_token("h", t).is_ok());
        assert!(
            state.take_pair_token("h", t).is_err(),
            "a spent token must not work again"
        );
    }

    #[test]
    fn a_pair_token_expires_after_ten_minutes() {
        let mut state = State::default();
        let t = now();
        state.add_pair_token("h".to_string(), t);
        let late = t + Duration::seconds(PAIR_TOKEN_TTL_SECS + 1);
        assert!(state.take_pair_token("h", late).is_err());
    }

    #[test]
    fn a_handoff_token_expires_after_a_minute_and_works_once() {
        let mut state = State::default();
        let t = now();
        state.add_handoff("a".to_string(), "dev".to_string(), t);
        state.add_handoff("b".to_string(), "dev".to_string(), t);
        assert_eq!(state.take_handoff("a", t).unwrap(), "dev");
        assert!(state.take_handoff("a", t).is_err());
        let late = t + Duration::seconds(HANDOFF_TTL_SECS + 1);
        assert!(state.take_handoff("b", late).is_err());
    }

    #[test]
    fn only_ten_requests_wait_at_once() {
        let mut state = State::default();
        let t = now();
        for i in 0..MAX_PENDING_REQUESTS {
            state
                .add_request(format!("h{i}"), "Phone".to_string(), t)
                .unwrap();
        }
        let refused = state.add_request("extra".to_string(), "Phone".to_string(), t);
        assert!(
            matches!(refused, Err(PairingError::RateLimited(_))),
            "the eleventh request must be refused: {refused:?}"
        );
        let later = t + Duration::seconds(REQUEST_TTL_SECS + 1);
        assert!(
            state
                .add_request("fresh".to_string(), "Phone".to_string(), later)
                .is_ok(),
            "expired requests free their place"
        );
    }

    #[test]
    fn an_approved_request_issues_a_credential_to_its_poller_once() {
        let mut state = State::default();
        let t = now();
        let secret_hash = secrets::hash("request-secret");
        let (public_id, code) = state
            .add_request(secret_hash.clone(), "Phone".to_string(), t)
            .unwrap();
        assert_eq!(code.len(), secrets::PAIRING_CODE_LEN);
        assert!(matches!(
            state.poll(&secret_hash, t).unwrap(),
            PollOutcome::Pending
        ));
        state.decide(&public_id, true, t).unwrap();
        let PollOutcome::Approved(issued) = state.poll(&secret_hash, t).unwrap() else {
            panic!("an approved request should issue a credential");
        };
        assert_eq!(issued.device_name, "Phone");
        assert!(state.authenticate(Surface::Ui, &issued.secret, t).is_some());
        assert!(
            matches!(state.poll(&secret_hash, t).unwrap(), PollOutcome::Expired),
            "the request is spent"
        );
    }

    #[test]
    fn a_refused_request_pairs_nothing() {
        let mut state = State::default();
        let t = now();
        let secret_hash = secrets::hash("s");
        let (public_id, _) = state
            .add_request(secret_hash.clone(), "Phone".to_string(), t)
            .unwrap();
        state.decide(&public_id, false, t).unwrap();
        assert!(matches!(
            state.poll(&secret_hash, t).unwrap(),
            PollOutcome::Denied
        ));
        assert!(state.persisted.devices.is_empty());
    }

    #[test]
    fn deciding_twice_or_on_nothing_is_rejected() {
        let mut state = State::default();
        let t = now();
        let (public_id, _) = state
            .add_request("h".to_string(), "Phone".to_string(), t)
            .unwrap();
        state.decide(&public_id, true, t).unwrap();
        assert!(state.decide(&public_id, false, t).is_err());
        assert!(state.decide("nope", true, t).is_err());
    }

    #[test]
    fn recovery_codes_work_once_and_ignore_formatting() {
        let mut state = State::default();
        let t = now();
        let codes = state.replace_recovery_codes(t).unwrap();
        assert_eq!(codes.len(), RECOVERY_CODE_COUNT);
        assert_eq!(state.recovery_codes_remaining(), RECOVERY_CODE_COUNT);
        let typed = codes
            .first()
            .unwrap()
            .to_ascii_lowercase()
            .replace('-', " ");
        assert!(state.take_recovery_code(&typed).is_ok());
        assert_eq!(state.recovery_codes_remaining(), RECOVERY_CODE_COUNT - 1);
        assert!(
            state.take_recovery_code(&typed).is_err(),
            "a used code must not work again"
        );
        assert!(state.take_recovery_code("AAAAAAAAAAAAAAAA").is_err());
        assert!(state.take_recovery_code("short").is_err());
    }

    #[test]
    fn regenerating_recovery_codes_voids_the_old_ones() {
        let mut state = State::default();
        let t = now();
        let old = state.replace_recovery_codes(t).unwrap();
        state.replace_recovery_codes(t).unwrap();
        assert!(state.take_recovery_code(old.first().unwrap()).is_err());
    }

    #[test]
    fn a_revoked_device_is_refused_on_both_hosts() {
        let mut state = State::default();
        let t = now();
        let ui = state.add_device("Laptop", t).unwrap();
        let wb = state
            .add_workbench_credential(&ui.device_id, None, t)
            .unwrap();
        assert!(state.authenticate(Surface::Ui, &ui.secret, t).is_some());
        assert!(
            state
                .authenticate(Surface::Workbench, &wb.secret, t)
                .is_some()
        );
        assert!(state.revoke(&ui.device_id));
        assert!(state.authenticate(Surface::Ui, &ui.secret, t).is_none());
        assert!(
            state
                .authenticate(Surface::Workbench, &wb.secret, t)
                .is_none()
        );
        assert!(!state.revoke(&ui.device_id), "nothing is left to revoke");
    }

    #[test]
    fn a_credential_only_works_on_its_own_host() {
        let mut state = State::default();
        let t = now();
        let ui = state.add_device("Laptop", t).unwrap();
        let wb = state
            .add_workbench_credential(&ui.device_id, None, t)
            .unwrap();
        assert!(
            state
                .authenticate(Surface::Workbench, &ui.secret, t)
                .is_none()
        );
        assert!(state.authenticate(Surface::Ui, &wb.secret, t).is_none());
    }

    #[test]
    fn a_browser_that_already_holds_a_workbench_credential_keeps_it() {
        let mut state = State::default();
        let t = now();
        let ui = state.add_device("Laptop", t).unwrap();
        let first = state
            .add_workbench_credential(&ui.device_id, None, t)
            .unwrap();
        let again = state
            .add_workbench_credential(&ui.device_id, Some(&first.secret), t)
            .unwrap();
        assert_eq!(first.secret, again.secret);
        assert_eq!(
            state
                .persisted
                .devices
                .first()
                .unwrap()
                .workbench_hashes
                .len(),
            1
        );
    }

    #[test]
    fn a_device_unused_for_400_days_is_no_longer_paired() {
        let mut state = State::default();
        let t = now();
        let issued = state.add_device("Laptop", t).unwrap();
        let later = t + Duration::days(DEVICE_LIFETIME_DAYS - 1);
        assert!(
            state
                .authenticate(Surface::Ui, &issued.secret, later)
                .is_some()
        );
        let much_later = later + Duration::days(DEVICE_LIFETIME_DAYS + 1);
        assert!(
            state
                .authenticate(Surface::Ui, &issued.secret, much_later)
                .is_none()
        );
    }

    #[test]
    fn using_a_credential_refreshes_its_cookie_once_a_day() {
        let mut state = State::default();
        let t = now();
        let issued = state.add_device("Laptop", t).unwrap();
        let soon = t + Duration::hours(1);
        assert!(
            !state
                .authenticate(Surface::Ui, &issued.secret, soon)
                .unwrap()
                .refresh
        );
        let next_day = t + Duration::hours(COOKIE_REFRESH_AFTER_HOURS + 1);
        assert!(
            state
                .authenticate(Surface::Ui, &issued.secret, next_day)
                .unwrap()
                .refresh
        );
        assert!(
            !state
                .authenticate(Surface::Ui, &issued.secret, next_day + Duration::minutes(5))
                .unwrap()
                .refresh,
            "the refresh just happened"
        );
    }

    #[test]
    fn use_is_saved_lazily() {
        let mut state = State::default();
        let t = now();
        let issued = state.add_device("Laptop", t).unwrap();
        let used = t + Duration::minutes(5);
        state.authenticate(Surface::Ui, &issued.secret, used);
        assert!(!state.flush_due(used));
        assert!(state.flush_due(used + Duration::minutes(FLUSH_AFTER_MINUTES)));
        state.mark_saved();
        assert!(!state.flush_due(used + Duration::hours(1)));
    }

    #[test]
    fn limits_name_what_was_hit() {
        let mut state = State::default();
        let t = now();
        for _ in 0..PER_PEER_LIMIT {
            state.check_limit(Some("1.1.1.1"), t).unwrap();
        }
        let refused = state.check_limit(Some("1.1.1.1"), t).unwrap_err();
        assert!(matches!(refused, PairingError::RateLimited(_)));
    }
}
