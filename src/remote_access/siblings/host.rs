//! The instance being joined: nonces it has handed out, the join requests
//! waiting for a person's decision, and the answers waiting to be fetched.
//!
//! Everything is in memory. A restart forgets pending requests, and the
//! person who started the join simply starts it again.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Duration, Utc};
use rand::RngCore as _;
use serde_json::Value;

use super::protocol::{
    JoinPayload, LIFETIME_SECS, NonceReply, PayloadError, PollReply, REQUEST_PATH, SubmitReply,
    confirmation_code,
};
use crate::remote_access::jws::{AccountSigner, JwsError, VerifiedJws, verify_jws};

/// Join requests that may wait for a decision at once.
pub(crate) const MAX_PENDING: usize = 5;
/// Nonces that may be outstanding at once; the oldest is dropped past this.
const MAX_NONCES: usize = 64;

/// What the host needs to know about itself to take part in a join.
pub(crate) struct HostContext {
    pub(crate) user: String,
    pub(crate) slug: String,
    pub(crate) base_domain: String,
    pub(crate) account: Arc<dyn AccountSigner>,
}

impl HostContext {
    /// The URL a join request must be signed for.
    fn request_url(&self) -> String {
        format!(
            "https://{}.{}.{}{REQUEST_PATH}",
            self.slug, self.user, self.base_domain
        )
    }
}

/// Why the host refused a join step, in words for the requester.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum HostError {
    /// This instance isn't set up for remote access, so it can't take joins.
    #[error(
        "this instance isn't set up for remote access yet, so it can't approve other instances"
    )]
    NotReady,
    /// The pending list is full.
    #[error(
        "this instance already has {MAX_PENDING} join requests waiting; approve or deny one first"
    )]
    TooManyPending,
    /// The request's signature or structure is wrong.
    #[error("the join request isn't valid: {0}")]
    Invalid(String),
    /// The nonce was never issued, was used, or expired.
    #[error("the join code has expired or was already used; start the join again")]
    UnknownNonce,
}

/// A join request waiting for a decision, as the person sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingView {
    pub(crate) approval_id: String,
    pub(crate) code: String,
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) expires_at: DateTime<Utc>,
}

/// A request that can be approved: everything the approval records.
#[derive(Debug, Clone)]
pub(crate) struct PendingRequest {
    pub(crate) join_id: String,
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) account_uri: String,
    pub(crate) account_jwk: Value,
    /// The key the requester issued for this instance to present.
    pub(crate) key: String,
}

enum Decision {
    Pending,
    Approved(PollReply),
    Denied,
}

struct Entry {
    join_id: String,
    approval_id: String,
    request: PendingRequest,
    code: String,
    decision: Decision,
    expires_at: DateTime<Utc>,
}

struct NonceEntry {
    nonce: String,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct State {
    nonces: Vec<NonceEntry>,
    entries: Vec<Entry>,
}

impl State {
    fn prune(&mut self, now: DateTime<Utc>) {
        self.nonces.retain(|n| n.expires_at > now);
        self.entries.retain(|e| e.expires_at > now);
    }

    fn pending_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.decision, Decision::Pending))
            .count()
    }
}

/// The join endpoints' state.
#[derive(Default)]
pub(crate) struct JoinHost {
    state: Mutex<State>,
    context: Mutex<Option<Arc<HostContext>>>,
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0_u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

impl JoinHost {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start (or stop, with `None`) taking joins. Set once this instance is
    /// pinned and knows who it is; cleared when the tunnel goes away.
    pub(crate) fn set_context(&self, context: Option<Arc<HostContext>>) {
        *self.context.lock().unwrap_or_else(PoisonError::into_inner) = context;
    }

    pub(crate) fn context(&self) -> Option<Arc<HostContext>> {
        self.context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `GET /_sibling/join/nonce`: a fresh single-use nonce.
    pub(crate) fn issue_nonce(&self, now: DateTime<Utc>) -> Result<NonceReply, HostError> {
        let context = self.context().ok_or(HostError::NotReady)?;
        let nonce = random_hex(16);
        let mut state = self.state();
        state.prune(now);
        if state.nonces.len() >= MAX_NONCES {
            state.nonces.remove(0);
        }
        state.nonces.push(NonceEntry {
            nonce: nonce.clone(),
            expires_at: now + Duration::seconds(LIFETIME_SECS),
        });
        Ok(NonceReply {
            nonce,
            slug: context.slug.clone(),
            account_uri: context.account.uri().to_string(),
            account_jwk: context.account.jwk(),
        })
    }

    /// `POST /_sibling/join`: take a signed request and queue it for a decision.
    pub(crate) fn submit(&self, jws: &Value, now: DateTime<Utc>) -> Result<SubmitReply, HostError> {
        let context = self.context().ok_or(HostError::NotReady)?;
        let verified = verify_jws(jws).map_err(|e| HostError::Invalid(e.to_string()))?;
        let payload = checked_payload(&verified, &context)?;
        let code = confirmation_code(&context.account.jwk(), &verified.jwk, &payload.nonce)
            .map_err(|e: JwsError| HostError::Invalid(e.to_string()))?;

        let mut state = self.state();
        state.prune(now);
        if state.pending_count() >= MAX_PENDING {
            return Err(HostError::TooManyPending);
        }
        let position = state
            .nonces
            .iter()
            .position(|n| n.nonce == payload.nonce)
            .ok_or(HostError::UnknownNonce)?;
        state.nonces.remove(position);

        let join_id = random_hex(16);
        state.entries.push(Entry {
            join_id: join_id.clone(),
            approval_id: random_hex(6),
            request: PendingRequest {
                join_id: join_id.clone(),
                slug: payload.slug,
                display_name: payload.display_name.trim().to_string(),
                account_uri: payload.account_uri,
                account_jwk: verified.jwk,
                key: payload.key,
            },
            code,
            decision: Decision::Pending,
            expires_at: now + Duration::seconds(LIFETIME_SECS),
        });
        Ok(SubmitReply { join_id })
    }

    /// `GET /_sibling/join/{join id}`: where the request stands. `None` for an
    /// id that was never issued or has expired.
    pub(crate) fn poll(&self, join_id: &str, now: DateTime<Utc>) -> Option<PollReply> {
        let mut state = self.state();
        state.prune(now);
        let entry = state.entries.iter().find(|e| e.join_id == join_id)?;
        Some(match &entry.decision {
            Decision::Pending => PollReply::Pending,
            Decision::Approved(reply) => reply.clone(),
            Decision::Denied => PollReply::Denied,
        })
    }

    /// The requests waiting for a decision, oldest first.
    pub(crate) fn pending(&self, now: DateTime<Utc>) -> Vec<PendingView> {
        let mut state = self.state();
        state.prune(now);
        state
            .entries
            .iter()
            .filter(|e| matches!(e.decision, Decision::Pending))
            .map(|e| PendingView {
                approval_id: e.approval_id.clone(),
                code: e.code.clone(),
                slug: e.request.slug.clone(),
                display_name: e.request.display_name.clone(),
                expires_at: e.expires_at,
            })
            .collect()
    }

    /// The pending request with this approval id, for the person's approval to act on.
    pub(crate) fn find_pending(
        &self,
        approval_id: &str,
        now: DateTime<Utc>,
    ) -> Option<PendingRequest> {
        let mut state = self.state();
        state.prune(now);
        state
            .entries
            .iter()
            .find(|e| e.approval_id == approval_id && matches!(e.decision, Decision::Pending))
            .map(|e| e.request.clone())
    }

    /// Record the approval: the requester's next poll receives `reply`.
    pub(crate) fn approve(&self, join_id: &str, reply: PollReply, now: DateTime<Utc>) {
        self.decide(join_id, Decision::Approved(reply), now);
    }

    /// Record a refusal. Returns whether a pending request had this approval id.
    pub(crate) fn deny(&self, approval_id: &str, now: DateTime<Utc>) -> bool {
        let Some(request) = self.find_pending(approval_id, now) else {
            return false;
        };
        self.decide(&request.join_id, Decision::Denied, now);
        true
    }

    fn decide(&self, join_id: &str, decision: Decision, now: DateTime<Utc>) {
        let mut state = self.state();
        if let Some(entry) = state.entries.iter_mut().find(|e| e.join_id == join_id) {
            entry.decision = decision;
            // The answer stays fetchable for the full lifetime from the decision.
            entry.expires_at = now + Duration::seconds(LIFETIME_SECS);
        }
    }
}

/// Parse and validate the signed request's payload against the host.
fn checked_payload(
    verified: &VerifiedJws,
    context: &HostContext,
) -> Result<JoinPayload, HostError> {
    if verified.protected.get("url").and_then(Value::as_str) != Some(context.request_url().as_str())
    {
        return Err(HostError::Invalid(
            "it was signed for a different address".to_string(),
        ));
    }
    let payload: JoinPayload = serde_json::from_value(verified.payload.clone())
        .map_err(|_unparsed| HostError::Invalid("it is missing fields".to_string()))?;
    payload
        .validate(&context.slug)
        .map_err(|e: PayloadError| HostError::Invalid(e.to_string()))?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::remote_access::jws::sign_jws;
    use crate::remote_access::jws::test_key::TestAccount;
    use crate::remote_access::siblings::keys::issue_key;

    const BASE: &str = "relay.test";

    fn host() -> (JoinHost, Arc<TestAccount>) {
        let account = Arc::new(TestAccount::new("https://acme.test/acct/host"));
        let host = JoinHost::default();
        host.set_context(Some(Arc::new(HostContext {
            user: "bear".into(),
            slug: "laptop".into(),
            base_domain: BASE.into(),
            account: Arc::clone(&account) as Arc<dyn AccountSigner>,
        })));
        (host, account)
    }

    fn request(requester: &TestAccount, nonce: &str, slug: &str) -> Value {
        let header = json!({
            "alg": "ES256",
            "url": format!("https://laptop.bear.{BASE}{REQUEST_PATH}"),
            "jwk": requester.jwk(),
        });
        let payload = json!({
            "nonce": nonce,
            "slug": slug,
            "display_name": "Desk",
            "account_uri": requester.uri(),
            "key": issue_key(),
        });
        sign_jws(requester, &header, &payload).unwrap()
    }

    #[test]
    fn a_nonce_is_single_use_and_expires() {
        let (host, _) = host();
        let now = Utc::now();
        let requester = TestAccount::new("https://acme.test/acct/r1");
        let nonce = host.issue_nonce(now).unwrap().nonce;
        host.submit(&request(&requester, &nonce, "desktop"), now)
            .unwrap();
        assert_eq!(
            host.submit(&request(&requester, &nonce, "desktop"), now),
            Err(HostError::UnknownNonce),
            "the nonce was spent"
        );
        let stale = host.issue_nonce(now).unwrap().nonce;
        let later = now + Duration::seconds(LIFETIME_SECS + 1);
        assert_eq!(
            host.submit(&request(&requester, &stale, "desktop"), later),
            Err(HostError::UnknownNonce)
        );
        assert!(
            host.issue_nonce(now).is_ok(),
            "the nonce route still answers after an expiry"
        );
    }

    #[test]
    fn the_code_differs_when_the_requester_key_differs() {
        let (host, _) = host();
        let now = Utc::now();
        let (honest, attacker) = (
            TestAccount::new("https://acme.test/acct/r1"),
            TestAccount::new("https://acme.test/acct/r2"),
        );
        // Both requests reuse one nonce value, as a man in the middle would have to.
        let first = host.issue_nonce(now).unwrap().nonce;
        host.submit(&request(&honest, &first, "desktop"), now)
            .unwrap();
        let second = host.issue_nonce(now).unwrap().nonce;
        host.submit(&request(&attacker, &second, "desktop"), now)
            .unwrap();
        let [honest_view, attacker_view] = host
            .pending(now)
            .as_slice()
            .to_vec()
            .try_into()
            .unwrap_or_else(|v: Vec<PendingView>| panic!("two pending requests, got {}", v.len()));

        let host_jwk = host.context().unwrap().account.jwk();
        let expected_honest = confirmation_code(&host_jwk, &honest.jwk(), &first).unwrap();
        let expected_attacker = confirmation_code(&host_jwk, &attacker.jwk(), &second).unwrap();
        assert_ne!(expected_honest, expected_attacker);
        assert_eq!(honest_view.code, expected_honest);
        assert_eq!(attacker_view.code, expected_attacker);
    }

    #[test]
    fn a_request_signed_for_another_address_or_tampered_with_is_refused() {
        let (host, _) = host();
        let now = Utc::now();
        let requester = TestAccount::new("https://acme.test/acct/r1");
        let nonce = host.issue_nonce(now).unwrap().nonce;
        let header = json!({
            "alg": "ES256",
            "url": format!("https://other.bear.{BASE}{REQUEST_PATH}"),
            "jwk": requester.jwk(),
        });
        let payload = json!({
            "nonce": nonce, "slug": "desktop", "display_name": "Desk",
            "account_uri": requester.uri(), "key": issue_key(),
        });
        let wrong_url = sign_jws(&requester, &header, &payload).unwrap();
        assert!(matches!(
            host.submit(&wrong_url, now),
            Err(HostError::Invalid(_))
        ));

        let mut tampered = request(&requester, &nonce, "desktop");
        let other = request(&requester, &nonce, "elsewhere");
        tampered
            .as_object_mut()
            .unwrap()
            .insert("payload".into(), other.get("payload").cloned().unwrap());
        assert!(matches!(
            host.submit(&tampered, now),
            Err(HostError::Invalid(_))
        ));
        assert!(host.pending(now).is_empty());
    }

    #[test]
    fn more_than_five_pending_requests_are_refused() {
        let (host, _) = host();
        let now = Utc::now();
        for n in 0..MAX_PENDING {
            let requester = TestAccount::new(&format!("https://acme.test/acct/r{n}"));
            let nonce = host.issue_nonce(now).unwrap().nonce;
            host.submit(&request(&requester, &nonce, &format!("inst-{n}")), now)
                .unwrap();
        }
        let extra = TestAccount::new("https://acme.test/acct/extra");
        let nonce = host.issue_nonce(now).unwrap().nonce;
        assert_eq!(
            host.submit(&request(&extra, &nonce, "one-too-many"), now),
            Err(HostError::TooManyPending)
        );
        // A refused request doesn't spend its nonce, and a decision frees a place.
        let first = host.pending(now).remove(0);
        assert!(host.deny(&first.approval_id, now));
        host.submit(&request(&extra, &nonce, "one-too-many"), now)
            .unwrap();
    }

    #[test]
    fn requests_expire_after_ten_minutes() {
        let (host, _) = host();
        let now = Utc::now();
        let requester = TestAccount::new("https://acme.test/acct/r1");
        let nonce = host.issue_nonce(now).unwrap().nonce;
        let join_id = host
            .submit(&request(&requester, &nonce, "desktop"), now)
            .unwrap()
            .join_id;
        assert_eq!(host.pending(now).len(), 1);
        let later = now + Duration::seconds(LIFETIME_SECS + 1);
        assert!(host.pending(later).is_empty());
        assert!(host.poll(&join_id, later).is_none());
    }

    #[test]
    fn polling_follows_the_decision_and_only_the_join_id_holder_can_poll() {
        let (host, account) = host();
        let now = Utc::now();
        let requester = TestAccount::new("https://acme.test/acct/r1");
        let nonce = host.issue_nonce(now).unwrap().nonce;
        let join_id = host
            .submit(&request(&requester, &nonce, "desktop"), now)
            .unwrap()
            .join_id;
        assert!(matches!(host.poll(&join_id, now), Some(PollReply::Pending)));
        assert!(host.poll("00000000000000000000000000000000", now).is_none());

        let pending = host.pending(now).remove(0);
        assert_ne!(
            pending.approval_id, join_id,
            "the id the approving person sees is not the one the requester polls with"
        );
        let request = host.find_pending(&pending.approval_id, now).unwrap();
        assert_eq!(request.join_id, join_id);
        host.approve(
            &join_id,
            PollReply::Approved {
                key: "k".into(),
                account_uri: account.uri().into(),
                account_jwk: account.jwk(),
            },
            now,
        );
        assert!(matches!(
            host.poll(&join_id, now),
            Some(PollReply::Approved { .. })
        ));
        assert!(
            host.pending(now).is_empty(),
            "a decided request no longer waits"
        );
    }

    #[test]
    fn nothing_is_taken_before_the_host_is_ready() {
        let host = JoinHost::default();
        assert_eq!(
            host.issue_nonce(Utc::now()).unwrap_err(),
            HostError::NotReady
        );
        let requester = TestAccount::new("https://acme.test/acct/r1");
        assert_eq!(
            host.submit(&request(&requester, "n", "desktop"), Utc::now()),
            Err(HostError::NotReady)
        );
    }
}
