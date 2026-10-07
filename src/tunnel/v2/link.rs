//! [`RelayLink`]: the handler's request channel to the relay over the live
//! tunnel (pin grants and challenge claims).

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::oneshot;

use super::frames::V2Frame;
use super::stream::Outbox;
use crate::remote_access::types::normalize_host;

/// How long a pin grant request waits for the relay's answer.
const GRANT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a challenge claim waits for the relay's answer.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(15);

/// Why a challenge claim did not succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimError {
    /// Another instance of the user holds the claim.
    Busy,
    /// The relay did not answer in time.
    Timeout,
    /// The tunnel ended.
    Closed,
}

const TUNNEL_LOST: &str = "the connection to the relay was lost";

#[derive(Clone, Copy)]
enum ClaimReply {
    Granted,
    Busy,
}

struct PendingClaim {
    id: u64,
    names: HashSet<String>,
    reply: oneshot::Sender<ClaimReply>,
}

#[derive(Default)]
struct Pending {
    closed: bool,
    next_id: u64,
    grants: HashMap<String, oneshot::Sender<Result<String, String>>>,
    claims: Vec<PendingClaim>,
}

pub(super) struct LinkShared {
    outbox: Outbox,
    pending: Mutex<Pending>,
}

impl LinkShared {
    pub(super) fn new(outbox: Outbox) -> Arc<Self> {
        Arc::new(Self {
            outbox,
            pending: Mutex::new(Pending::default()),
        })
    }

    fn pending(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The relay answered a grant request.
    pub(super) fn resolve_grant(&self, purpose: &str, result: Result<String, String>) {
        let waiter = self.pending().grants.remove(purpose);
        let Some(tx) = waiter else {
            tracing::warn!(purpose, "relay sent a pin grant answer nobody asked for");
            return;
        };
        if tx.send(result).is_err() {
            tracing::debug!(purpose, "grant requester gave up before the answer");
        }
    }

    /// The relay answered a challenge claim.
    pub(super) fn resolve_claim(&self, names: &[String], granted: bool) {
        let wanted: HashSet<String> = names.iter().map(|n| normalize_host(n)).collect();
        let waiter = {
            let mut pending = self.pending();
            let position = pending.claims.iter().position(|c| c.names == wanted);
            position.map(|i| pending.claims.remove(i))
        };
        let Some(claim) = waiter else {
            tracing::warn!("relay answered a challenge claim nobody made");
            return;
        };
        let reply = if granted {
            ClaimReply::Granted
        } else {
            ClaimReply::Busy
        };
        if claim.reply.send(reply).is_err() {
            tracing::debug!("claim requester gave up before the answer");
        }
    }

    /// The tunnel ended: every waiter fails now and later requests fail fast.
    pub(super) fn close(&self) {
        let mut pending = self.pending();
        pending.closed = true;
        pending.grants.clear();
        pending.claims.clear();
    }
}

/// The handler's way to ask the relay for things over the live tunnel.
#[derive(Clone)]
pub(crate) struct RelayLink {
    shared: Arc<LinkShared>,
}

impl RelayLink {
    pub(super) fn new(shared: Arc<LinkShared>) -> Self {
        Self { shared }
    }

    /// Ask the relay for a pin-service grant for `purpose` (`enroll` or
    /// `reset`).
    ///
    /// # Errors
    ///
    /// The relay's reason when it refuses, or why no answer came (timeout,
    /// tunnel lost, a request for this purpose already in flight).
    pub(crate) async fn request_grant(&self, purpose: &str) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.shared.pending();
            if pending.closed {
                return Err(TUNNEL_LOST.to_string());
            }
            if pending.grants.contains_key(purpose) {
                return Err(format!(
                    "a {purpose} grant request is already waiting for the relay"
                ));
            }
            pending.grants.insert(purpose.to_string(), tx);
        }
        self.shared.outbox.send_control(&V2Frame::PinGrantRequest {
            purpose: purpose.to_string(),
        });
        match tokio::time::timeout(GRANT_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(TUNNEL_LOST.to_string()),
            Err(_) => {
                self.shared.pending().grants.remove(purpose);
                Err("the relay did not answer the grant request in time".to_string())
            }
        }
    }

    /// Ask the relay to route TLS-ALPN-01 validation for `names` here.
    ///
    /// # Errors
    ///
    /// [`ClaimError`] when another instance holds the claim, the relay is
    /// silent, or the tunnel is gone.
    pub(crate) async fn claim_challenge(&self, names: &[String]) -> Result<(), ClaimError> {
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut pending = self.shared.pending();
            if pending.closed {
                return Err(ClaimError::Closed);
            }
            pending.next_id += 1;
            let id = pending.next_id;
            pending.claims.push(PendingClaim {
                id,
                names: names.iter().map(|n| normalize_host(n)).collect(),
                reply: tx,
            });
            id
        };
        self.shared.outbox.send_control(&V2Frame::ChallengeClaim {
            names: names.to_vec(),
        });
        match tokio::time::timeout(CLAIM_TIMEOUT, rx).await {
            Ok(Ok(ClaimReply::Granted)) => Ok(()),
            Ok(Ok(ClaimReply::Busy)) => Err(ClaimError::Busy),
            Ok(Err(_)) => Err(ClaimError::Closed),
            Err(_) => {
                self.shared.pending().claims.retain(|c| c.id != id);
                Err(ClaimError::Timeout)
            }
        }
    }

    /// Tell the relay validation for `names` no longer comes here.
    pub(crate) fn release_challenge(&self, names: &[String]) {
        if self.shared.pending().closed {
            tracing::debug!("challenge release skipped: the tunnel is gone");
            return;
        }
        self.shared.outbox.send_control(&V2Frame::ChallengeRelease {
            names: names.to_vec(),
        });
    }

    /// Ask the relay to make `slug` the user's active instance. The relay
    /// answers with a new instance list, not with a reply to this.
    pub(crate) fn activate_instance(&self, slug: &str) {
        if self.shared.pending().closed {
            tracing::debug!("instance switch skipped: the tunnel is gone");
            return;
        }
        self.shared.outbox.send_control(&V2Frame::ActivateInstance {
            slug: slug.to_string(),
        });
    }

    /// Whether the tunnel this link belongs to has ended.
    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        self.shared.pending().closed
    }
}

#[cfg(test)]
mod tests {
    use tokio_tungstenite::tungstenite::Message;

    use super::*;
    use crate::tunnel::v2::stream::OutboxReceivers;

    fn link() -> (RelayLink, Arc<LinkShared>, OutboxReceivers) {
        let (outbox, rx) = Outbox::new();
        let shared = LinkShared::new(outbox);
        (RelayLink::new(Arc::clone(&shared)), shared, rx)
    }

    async fn sent(rx: &mut OutboxReceivers) -> String {
        let Message::Text(text) = rx.control.recv().await.unwrap() else {
            panic!("expected text");
        };
        text.as_str().to_string()
    }

    #[tokio::test]
    async fn grant_round_trip() {
        let (link, shared, mut rx) = link();
        let task = tokio::spawn({
            let link = link.clone();
            async move { link.request_grant("enroll").await }
        });
        assert_eq!(
            sent(&mut rx).await,
            r#"{"type":"pin_grant_request","purpose":"enroll"}"#
        );
        shared.resolve_grant("enroll", Ok("jws".into()));
        assert_eq!(task.await.unwrap(), Ok("jws".to_string()));
    }

    #[tokio::test]
    async fn grant_error_reason_is_returned() {
        let (link, shared, mut rx) = link();
        let task = tokio::spawn({
            let link = link.clone();
            async move { link.request_grant("reset").await }
        });
        sent(&mut rx).await;
        shared.resolve_grant("reset", Err("nope".into()));
        assert_eq!(task.await.unwrap(), Err("nope".to_string()));
    }

    #[tokio::test]
    async fn only_one_grant_request_per_purpose_is_in_flight() {
        let (link, shared, mut rx) = link();
        let first = tokio::spawn({
            let link = link.clone();
            async move { link.request_grant("enroll").await }
        });
        sent(&mut rx).await;
        assert!(link.request_grant("enroll").await.is_err());
        shared.resolve_grant("enroll", Ok("g".into()));
        assert!(first.await.unwrap().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn grant_times_out() {
        let (link, _shared, _rx) = link();
        let result = link.request_grant("enroll").await;
        assert!(result.unwrap_err().contains("in time"));
    }

    #[tokio::test]
    async fn claim_granted_and_busy() {
        let (link, shared, mut rx) = link();
        let names = vec!["Bear.Example.com".to_string()];
        let granted_task = tokio::spawn({
            let (link, names) = (link.clone(), names.clone());
            async move { link.claim_challenge(&names).await }
        });
        assert!(sent(&mut rx).await.contains("challenge_claim"));
        shared.resolve_claim(&["bear.example.com".to_string()], true);
        assert_eq!(granted_task.await.unwrap(), Ok(()));

        let busy_task = tokio::spawn({
            let (link, names) = (link.clone(), names.clone());
            async move { link.claim_challenge(&names).await }
        });
        sent(&mut rx).await;
        shared.resolve_claim(&names, false);
        assert_eq!(busy_task.await.unwrap(), Err(ClaimError::Busy));
    }

    #[tokio::test]
    async fn closing_fails_waiters_and_later_requests_fast() {
        let (link, shared, mut rx) = link();
        let grant = tokio::spawn({
            let link = link.clone();
            async move { link.request_grant("enroll").await }
        });
        let claim = tokio::spawn({
            let link = link.clone();
            async move { link.claim_challenge(&["a.example".to_string()]).await }
        });
        sent(&mut rx).await;
        sent(&mut rx).await;
        shared.close();
        assert!(grant.await.unwrap().is_err());
        assert_eq!(claim.await.unwrap(), Err(ClaimError::Closed));
        assert_eq!(
            link.claim_challenge(&["a.example".to_string()]).await,
            Err(ClaimError::Closed)
        );
        assert!(link.request_grant("reset").await.is_err());
        assert!(link.is_closed());
    }

    #[tokio::test(start_paused = true)]
    async fn claim_times_out() {
        let (link, _shared, _rx) = link();
        assert_eq!(
            link.claim_challenge(&["a.example".to_string()]).await,
            Err(ClaimError::Timeout)
        );
    }
}
