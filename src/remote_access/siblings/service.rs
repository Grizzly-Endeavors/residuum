//! Both halves of a join as operations: approving a request on the instance
//! being joined, and running a join from the instance that asks.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use super::client::{
    JoinFailure, JoinRequest, JoinSubmitted, PollOutcome, SiblingChannel, poll_join, submit_join,
};
use super::host::{JoinHost, PendingRequest};
use super::keys::{NewSibling, SiblingKeys, issue_key};
use super::protocol::PollReply;
use crate::remote_access::jws::AccountSigner;
use crate::remote_access::pins::{NewPin, PinClient, PinError};
use crate::remote_access::status::{JoinProgress, JoinState};
use crate::remote_access::store::StateStore;

/// How a join waits for the other side's person.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JoinTimings {
    pub(crate) poll_every: Duration,
    pub(crate) give_up_after: Duration,
}

impl Default for JoinTimings {
    fn default() -> Self {
        Self {
            poll_every: Duration::from_secs(3),
            give_up_after: Duration::from_secs(super::protocol::LIFETIME_SECS.unsigned_abs()),
        }
    }
}

/// Why an approval didn't happen.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ApproveError {
    /// No request is waiting under that id (decided, expired, or never there).
    #[error(
        "That join request isn't waiting any more. It may have expired; ask the other instance to start the join again."
    )]
    Unknown,
    /// The pin service refused or couldn't be reached; the request stays waiting.
    #[error(
        "The certificate account couldn't be added: {0} The request is still waiting; try approving again."
    )]
    Pins(PinError),
    /// The keys couldn't be saved; the request stays waiting.
    #[error(
        "Residuum couldn't save the new sibling: {0:#} The request is still waiting; try approving again."
    )]
    Storage(anyhow::Error),
}

/// What an approval needs from the instance approving.
pub(crate) struct Approver<'a> {
    pub(crate) signer: &'a dyn AccountSigner,
    pub(crate) pins: &'a PinClient,
    pub(crate) store: &'a StateStore,
    pub(crate) user: &'a str,
}

/// The join operations of one instance.
#[derive(Clone)]
pub(crate) struct SiblingService {
    pub(crate) host: Arc<JoinHost>,
    pub(crate) keys: Arc<SiblingKeys>,
    channel: Arc<dyn SiblingChannel>,
    timings: JoinTimings,
}

impl SiblingService {
    pub(crate) fn new(
        host: Arc<JoinHost>,
        keys: Arc<SiblingKeys>,
        channel: Arc<dyn SiblingChannel>,
    ) -> Self {
        Self {
            host,
            keys,
            channel,
            timings: JoinTimings::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_timings(mut self, timings: JoinTimings) -> Self {
        self.timings = timings;
        self
    }

    /// Approve the request waiting under `approval_id`: pin the requester's
    /// certificate account when it isn't pinned yet, record both keys, and
    /// let the requester's next poll collect this instance's key.
    pub(crate) async fn approve(
        &self,
        approver: &Approver<'_>,
        approval_id: &str,
    ) -> Result<PendingRequest, ApproveError> {
        let request = self
            .host
            .find_pending(approval_id, Utc::now())
            .ok_or(ApproveError::Unknown)?;

        let pinned = approver
            .pins
            .list(approver.user)
            .await
            .map_err(ApproveError::Pins)?;
        if !pinned
            .iter()
            .any(|pin| pin.account_uri == request.account_uri)
        {
            approver
                .pins
                .add(
                    approver.signer,
                    approver.user,
                    &NewPin {
                        slug: &request.slug,
                        account_uri: &request.account_uri,
                        jwk: &request.account_jwk,
                    },
                )
                .await
                .map_err(ApproveError::Pins)?;
        }

        let issued = issue_key();
        self.keys
            .upsert(NewSibling {
                slug: request.slug.clone(),
                display_name: request.display_name.clone(),
                account_uri: request.account_uri.clone(),
                outbound_key: request.key.clone(),
                inbound_key: issued.clone(),
            })
            .await
            .map_err(ApproveError::Storage)?;
        approver
            .store
            .add_known(&request.account_uri)
            .await
            .map_err(ApproveError::Storage)?;

        self.host.approve(
            &request.join_id,
            PollReply::Approved {
                key: issued,
                account_uri: approver.signer.uri().to_string(),
                account_jwk: approver.signer.jwk(),
            },
            Utc::now(),
        );
        Ok(request)
    }

    /// Start a join with `target_slug` and follow it until it is decided.
    /// `progress` hears each change; the result says how it ended.
    pub(crate) async fn join(
        &self,
        joiner: &Joiner<'_>,
        progress: &(dyn Fn(JoinProgress) + Send + Sync),
    ) -> Result<(), JoinFailure> {
        let target = joiner.target_slug;
        let report = |state, code: Option<&str>, detail: Option<String>| {
            progress(JoinProgress {
                instance: target.to_string(),
                state,
                code: code.map(str::to_string),
                detail,
            });
        };
        let request = JoinRequest {
            account: joiner.signer,
            user: joiner.user,
            own_slug: joiner.own_slug,
            display_name: joiner.display_name,
            base_domain: joiner.base_domain,
            target_slug: target,
        };
        let submitted = match submit_join(self.channel.as_ref(), &request, issue_key()).await {
            Ok(submitted) => submitted,
            Err(failure) => {
                report(JoinState::Failed, None, Some(failure.to_string()));
                return Err(failure);
            }
        };
        report(
            JoinState::Waiting,
            Some(&submitted.code),
            Some(format!(
                "Waiting for someone to approve this on \"{target}\". Check that it shows the code {}.",
                submitted.code
            )),
        );
        match self.await_approval(target, &submitted).await {
            Ok(host_key) => match self.record(joiner, &submitted, host_key).await {
                Ok(()) => {
                    report(
                        JoinState::Approved,
                        Some(&submitted.code),
                        Some(format!("\"{target}\" approved this instance.")),
                    );
                    Ok(())
                }
                Err(failure) => {
                    report(
                        JoinState::Failed,
                        Some(&submitted.code),
                        Some(failure.to_string()),
                    );
                    Err(failure)
                }
            },
            Err(failure) => {
                let state = if matches!(failure, JoinFailure::Denied { .. }) {
                    JoinState::Denied
                } else {
                    JoinState::Failed
                };
                report(state, Some(&submitted.code), Some(failure.to_string()));
                Err(failure)
            }
        }
    }

    async fn await_approval(
        &self,
        target: &str,
        submitted: &JoinSubmitted,
    ) -> Result<String, JoinFailure> {
        let deadline = tokio::time::Instant::now() + self.timings.give_up_after;
        loop {
            match poll_join(self.channel.as_ref(), target, submitted).await? {
                PollOutcome::Approved { key } => return Ok(key),
                PollOutcome::Waiting => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(JoinFailure::Expired {
                    slug: target.to_string(),
                });
            }
            tokio::time::sleep(self.timings.poll_every).await;
        }
    }

    /// Store the join once the pin service confirms the other instance is
    /// pinned under the account it named.
    async fn record(
        &self,
        joiner: &Joiner<'_>,
        submitted: &JoinSubmitted,
        host_key: String,
    ) -> Result<(), JoinFailure> {
        let target = joiner.target_slug;
        let refused = |reason: String| JoinFailure::Refused {
            slug: target.to_string(),
            reason,
        };
        let pinned = joiner
            .pins
            .list(joiner.user)
            .await
            .map_err(|e| refused(format!("the pin service couldn't confirm it: {e}")))?;
        if !pinned
            .iter()
            .any(|pin| pin.account_uri == submitted.host_account_uri && pin.slug == target)
        {
            return Err(refused(
                "the pin service doesn't list its certificate account, so it can't be trusted as a sibling"
                    .to_string(),
            ));
        }
        self.keys
            .upsert(NewSibling {
                slug: target.to_string(),
                display_name: target.to_string(),
                account_uri: submitted.host_account_uri.clone(),
                outbound_key: host_key,
                inbound_key: submitted.issued_key.clone(),
            })
            .await
            .map_err(|e| refused(format!("Residuum couldn't save it: {e:#}")))?;
        joiner
            .store
            .add_known(&submitted.host_account_uri)
            .await
            .map_err(|e| refused(format!("Residuum couldn't save it: {e:#}")))?;
        Ok(())
    }
}

/// What the instance asking to join brings.
pub(crate) struct Joiner<'a> {
    pub(crate) signer: &'a dyn AccountSigner,
    pub(crate) pins: &'a PinClient,
    pub(crate) store: &'a StateStore,
    pub(crate) user: &'a str,
    pub(crate) own_slug: &'a str,
    pub(crate) display_name: &'a str,
    pub(crate) base_domain: &'a str,
    pub(crate) target_slug: &'a str,
}

#[cfg(test)]
mod tests {
    use axum::Router;

    use super::*;
    use crate::remote_access::fake_pin_service::FakePinService;
    use crate::remote_access::jws::test_key::TestAccount;
    use crate::remote_access::siblings::client::in_process::InProcess;
    use crate::remote_access::siblings::host::HostContext;
    use crate::remote_access::siblings::keys::SiblingKeyVerifier as _;
    use crate::remote_access::siblings::routes;

    const FAST: JoinTimings = JoinTimings {
        poll_every: Duration::from_millis(20),
        give_up_after: Duration::from_secs(5),
    };

    /// One instance: its account, stores and join machinery.
    struct Instance {
        account: Arc<TestAccount>,
        slug: &'static str,
        keys: Arc<SiblingKeys>,
        store: StateStore,
        service: SiblingService,
        router: Router,
        _dir: tempfile::TempDir,
    }

    impl Instance {
        /// `reach` is the router of the instance this one will call.
        fn new(slug: &'static str, reach: Option<Router>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let account = Arc::new(TestAccount::new(&format!("https://acme.test/acct/{slug}")));
            let keys = Arc::new(SiblingKeys::open(dir.path()));
            let host = Arc::new(JoinHost::default());
            host.set_context(Some(Arc::new(HostContext {
                user: "bear".into(),
                slug: slug.into(),
                base_domain: "relay.test".into(),
                account: Arc::clone(&account) as Arc<dyn AccountSigner>,
            })));
            let channel = InProcess::new(reach.unwrap_or_default(), "203.0.113.7");
            let service =
                SiblingService::new(Arc::clone(&host), Arc::clone(&keys), Arc::new(channel))
                    .with_timings(FAST);
            Self {
                account,
                slug,
                keys,
                store: StateStore::open(dir.path()),
                service,
                router: routes::router(host),
                _dir: dir,
            }
        }

        fn joiner<'a>(&'a self, pins: &'a PinClient, target: &'a str) -> Joiner<'a> {
            Joiner {
                signer: self.account.as_ref(),
                pins,
                store: &self.store,
                user: "bear",
                own_slug: self.slug,
                display_name: "Desk",
                base_domain: "relay.test",
                target_slug: target,
            }
        }

        fn approver<'a>(&'a self, pins: &'a PinClient) -> Approver<'a> {
            Approver {
                signer: self.account.as_ref(),
                pins,
                store: &self.store,
                user: "bear",
            }
        }

        async fn wait_for_pending(&self) -> String {
            crate::testing::wait::until("a join request to arrive", || {
                std::future::ready(
                    self.service
                        .host
                        .pending(Utc::now())
                        .first()
                        .map(|pending| pending.approval_id.clone()),
                )
            })
            .await
        }
    }

    /// `laptop` is the instance being joined and is pinned; `desktop` joins it.
    async fn world() -> (Instance, Instance, FakePinService, PinClient) {
        let laptop_probe = Instance::new("laptop", None);
        let desktop = Instance::new("desktop", Some(laptop_probe.router.clone()));
        let pins = FakePinService::start(None).await;
        pins.preload_with_key(
            laptop_probe.account.uri(),
            "laptop",
            laptop_probe.account.jwk(),
        );
        let client = PinClient::new(pins.url()).unwrap();
        (laptop_probe, desktop, pins, client)
    }

    fn progress_log() -> (
        Arc<std::sync::Mutex<Vec<JoinProgress>>>,
        impl Fn(JoinProgress) + Send + Sync,
    ) {
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        (log, move |p| sink.lock().unwrap().push(p))
    }

    #[tokio::test]
    async fn approval_pins_the_requester_and_exchanges_keys_both_ways() {
        let (laptop, desktop, pins, client) = world().await;
        let (log, progress) = progress_log();
        let asking = desktop.joiner(&client, "laptop");
        let join = desktop.service.join(&asking, &progress);
        let approve = async {
            let id = laptop.wait_for_pending().await;
            let shown = laptop.service.host.pending(Utc::now()).remove(0);
            // The code the approving person sees is the one the joiner reports.
            let reported =
                crate::testing::wait::until("the joiner to report the approval code", || {
                    std::future::ready(log.lock().unwrap().iter().find_map(|p| p.code.clone()))
                })
                .await;
            assert_eq!(shown.code, reported);
            laptop.service.approve(&laptop.approver(&client), &id).await
        };
        let (outcome, approved) = tokio::join!(join, approve);
        outcome.unwrap();
        let request = approved.unwrap();
        assert_eq!(request.slug, "desktop");

        // The pin service now lists the requester, added by the approver.
        assert_eq!(pins.operations(), ["add"]);
        assert!(
            pins.pins()
                .iter()
                .any(|(uri, slug)| uri == desktop.account.uri() && slug == "desktop")
        );
        // Each side knows the other's certificate account.
        assert!(laptop.store.is_known(desktop.account.uri()));
        assert!(desktop.store.is_known(laptop.account.uri()));

        // The key each side presents is the key the other accepts, tagged with the caller's slug.
        let laptop_to_desktop = laptop.keys.outbound_key("desktop").unwrap();
        let desktop_to_laptop = desktop.keys.outbound_key("laptop").unwrap();
        assert_eq!(
            desktop.keys.verify(&laptop_to_desktop).as_deref(),
            Some("laptop")
        );
        assert_eq!(
            laptop.keys.verify(&desktop_to_laptop).as_deref(),
            Some("desktop")
        );
        assert_eq!(laptop.keys.verify(&laptop_to_desktop), None);

        let states: Vec<JoinState> = log.lock().unwrap().iter().map(|p| p.state).collect();
        assert_eq!(states.first(), Some(&JoinState::Waiting));
        assert_eq!(states.last(), Some(&JoinState::Approved));
    }

    #[tokio::test]
    async fn a_requester_that_is_already_pinned_is_not_added_again() {
        let (laptop, desktop, pins, client) = world().await;
        pins.preload("https://acme.test/acct/desktop", "desktop");
        let (_log, progress) = progress_log();
        let asking = desktop.joiner(&client, "laptop");
        let (outcome, approved) = tokio::join!(desktop.service.join(&asking, &progress), async {
            let id = laptop.wait_for_pending().await;
            laptop.service.approve(&laptop.approver(&client), &id).await
        });
        outcome.unwrap();
        approved.unwrap();
        assert!(pins.operations().is_empty(), "{:?}", pins.operations());
    }

    #[tokio::test]
    async fn a_slow_approval_outlasts_the_join_rate_limit() {
        // The requester keeps polling while the person decides; polls must
        // not count against the per-peer limit on join requests.
        let (laptop, desktop, _pins, client) = world().await;
        let (_log, progress) = progress_log();
        let asking = desktop.joiner(&client, "laptop");
        let (outcome, approved) = tokio::join!(desktop.service.join(&asking, &progress), async {
            let id = laptop.wait_for_pending().await;
            // More polls than the per-peer limit allows join requests in a
            // minute (ten), all within that minute.
            crate::testing::wait::until("the requester to poll more than ten times", || {
                std::future::ready(
                    laptop
                        .service
                        .host
                        .pending(Utc::now())
                        .iter()
                        .any(|pending| pending.approval_id == id && pending.polls > 10)
                        .then_some(()),
                )
            })
            .await;
            laptop.service.approve(&laptop.approver(&client), &id).await
        });
        outcome.unwrap();
        approved.unwrap();
    }

    #[tokio::test]
    async fn a_denied_join_stores_nothing_and_says_so() {
        let (laptop, desktop, pins, client) = world().await;
        let (log, progress) = progress_log();
        let asking = desktop.joiner(&client, "laptop");
        let (outcome, ()) = tokio::join!(desktop.service.join(&asking, &progress), async {
            let id = laptop.wait_for_pending().await;
            assert!(laptop.service.host.deny(&id, Utc::now()));
        });
        assert!(matches!(outcome, Err(JoinFailure::Denied { .. })));
        assert!(desktop.keys.joined().is_empty());
        assert!(laptop.keys.joined().is_empty());
        assert!(pins.operations().is_empty());
        assert_eq!(log.lock().unwrap().last().unwrap().state, JoinState::Denied);
    }

    #[tokio::test]
    async fn an_approval_the_pin_service_refuses_leaves_the_request_waiting() {
        let (laptop, desktop, pins, _approver_pins) = world().await;
        // The approver talks to a pin service that doesn't know its key, so the add is refused.
        let strangers = FakePinService::start(None).await;
        let refusing = PinClient::new(strangers.url()).unwrap();
        let joiner_pins = PinClient::new(pins.url()).unwrap();
        let (_log, progress) = progress_log();
        let asking = desktop.joiner(&joiner_pins, "laptop");
        // The join keeps waiting for the approval, so it is dropped once the
        // refused approval has been tried and must not have finished first.
        let (id, attempt) = tokio::select! {
            _ = desktop.service.join(&asking, &progress) => {
                panic!("the join finished before the approval was tried");
            }
            pair = async {
                let id = laptop.wait_for_pending().await;
                let attempt = laptop
                    .service
                    .approve(&laptop.approver(&refusing), &id)
                    .await;
                (id, attempt)
            } => pair,
        };
        assert!(matches!(attempt, Err(ApproveError::Pins(_))));
        assert!(
            laptop.service.host.find_pending(&id, Utc::now()).is_some(),
            "the request is still waiting for another try"
        );
        assert!(laptop.keys.joined().is_empty());
        assert!(!laptop.store.is_known(desktop.account.uri()));
    }

    #[tokio::test]
    async fn approving_an_unknown_request_is_an_error() {
        let (laptop, _desktop, _pins, client) = world().await;
        let result = laptop
            .service
            .approve(&laptop.approver(&client), "deadbeefdead")
            .await;
        assert!(matches!(result, Err(ApproveError::Unknown)));
    }

    #[tokio::test]
    async fn the_joiner_trusts_nothing_the_pin_service_does_not_list() {
        let (laptop, desktop, _pins, _client) = world().await;
        // This pin service has no pin for the instance being joined.
        let empty = FakePinService::start(None).await;
        let empty_client = PinClient::new(empty.url()).unwrap();
        let (log, progress) = progress_log();
        let asking = desktop.joiner(&empty_client, "laptop");
        let (outcome, ()) = tokio::join!(desktop.service.join(&asking, &progress), async {
            let id = laptop.wait_for_pending().await;
            // Approve without going through the pin service, as a liar would.
            let request = laptop.service.host.find_pending(&id, Utc::now()).unwrap();
            laptop.service.host.approve(
                &request.join_id,
                PollReply::Approved {
                    key: issue_key(),
                    account_uri: laptop.account.uri().to_string(),
                    account_jwk: laptop.account.jwk(),
                },
                Utc::now(),
            );
        });
        assert!(matches!(outcome, Err(JoinFailure::Refused { .. })));
        assert!(desktop.keys.joined().is_empty());
        assert!(!desktop.store.is_known(laptop.account.uri()));
        assert_eq!(log.lock().unwrap().last().unwrap().state, JoinState::Failed);
    }
}
