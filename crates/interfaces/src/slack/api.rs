//! Web API client for Slack plus the rate-limited sender that wraps it.
//!
//! Thin by design (design doc, module table): `chat.postMessage` and the
//! `reactions.add`/`reactions.remove` pair behind the [`SlackApi`] trait,
//! and the minimum-interval sender that serializes all three. Knows nothing
//! about sockets or conversations — [`super::events`] and the socket task
//! own those.

use anyhow::Context as _;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::Mutex;

/// `chat.postMessage` — posts a message, optionally as a thread reply.
const POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";
/// `reactions.add` — the working reaction on the owner's message
/// (`reactions:write`).
const REACTIONS_ADD_URL: &str = "https://slack.com/api/reactions.add";
/// `reactions.remove` — clears the working reaction when the turn ends.
const REACTIONS_REMOVE_URL: &str = "https://slack.com/api/reactions.remove";
/// `apps.connections.open` — Socket Mode handshake, returns the WebSocket URL.
const CONNECTIONS_OPEN_URL: &str = "https://slack.com/api/apps.connections.open";

/// Per-request timeout for Slack Web API calls, matching the Teams adapter's
/// budget for its Microsoft-side HTTP.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Most 429 backoff rounds one call rides out before surfacing the rate
/// limit as an error. Sustained 429s must surface (design, failure mode 6),
/// never throttle silently.
const MAX_RATE_LIMIT_RETRIES: usize = 3;

/// What can go wrong on a Slack Web API call.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SlackApiError {
    /// Transport failure: DNS, connect, timeout — or a body that isn't the
    /// JSON document Slack always sends, meaning we never reached a real
    /// answer (proxy 502 HTML and friends).
    #[error("http error talking to slack: {0}")]
    Http(anyhow::Error),
    /// HTTP 429. The sender honors `retry_after` and retries; it only
    /// surfaces to the conversation layer when retries are exhausted.
    #[error("slack is rate limiting us; retry after {retry_after:?}")]
    RateLimited {
        /// How long Slack said to wait, from the `Retry-After` header.
        retry_after: Duration,
    },
    /// Slack answered `ok:false` — a rejection no retry fixes (bad channel,
    /// bad token, …). Surfaced verbatim.
    #[error("slack api rejected the call: {0}")]
    Fatal(String),
}

/// The slice of the Slack Web API the interface needs, behind a trait so the
/// sender (and its tests) never need the network.
#[async_trait]
pub(crate) trait SlackApi: Send + Sync {
    /// Post `text` to `channel` via `chat.postMessage` — as a reply in the
    /// thread rooted at `thread_ts` when one is given, otherwise at the top
    /// level of the conversation.
    ///
    /// # Errors
    /// [`SlackApiError::RateLimited`] on HTTP 429 — the caller is expected to
    /// honor `retry_after`; `Fatal` for Slack-side `ok:false` rejections;
    /// `Http` for transport failures.
    async fn post_message(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<(), SlackApiError>;

    /// Add the reaction `name` (an emoji shortcode without colons) to the
    /// message `ts` in `channel` via `reactions.add`. Slack's
    /// `already_reacted` counts as success: the reaction is there.
    ///
    /// # Errors
    /// Same contract as [`Self::post_message`].
    async fn add_reaction(&self, channel: &str, ts: &str, name: &str) -> Result<(), SlackApiError>;

    /// Remove the bot's reaction `name` from the message `ts` in `channel`
    /// via `reactions.remove`. Slack's `no_reaction` counts as success: the
    /// reaction is gone either way.
    ///
    /// # Errors
    /// Same contract as [`Self::post_message`].
    async fn remove_reaction(
        &self,
        channel: &str,
        ts: &str,
        name: &str,
    ) -> Result<(), SlackApiError>;

    /// Open a Socket Mode WebSocket via `apps.connections.open`, returning
    /// its URL.
    ///
    /// # Errors
    /// Any transport or Slack-side failure; the socket task's reconnect loop
    /// owns retrying this one.
    async fn open_connection(&self) -> anyhow::Result<String>;
}

/// [`SlackApi`] backed by the real Slack Web API.
///
/// `Clone` is cheap (the reqwest client is handle-based): the interface
/// constructor clones one instance into the boot-time connect probe and the
/// socket task keeps the original — both talk to the same Slack app.
#[derive(Clone)]
pub(crate) struct ReqwestSlackApi {
    http: reqwest::Client,
    bot_token: String,
    app_token: String,
}

impl ReqwestSlackApi {
    /// Build the adapter with its own client — constructed once per interface
    /// instance by Task 7's interface constructor, then shared.
    ///
    /// # Errors
    /// Fails only if the reqwest client itself cannot be built (TLS setup).
    pub(crate) fn new(bot_token: String, app_token: String) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .context("failed to build slack http client")?;
        Ok(Self {
            http,
            bot_token,
            app_token,
        })
    }

    /// POST a JSON body to a bot-token Web API method and interpret the
    /// answer — the shared front half of every `chat.*`/`reactions.*` call.
    async fn call(
        &self,
        url: &str,
        body: &Value,
        interpret: fn(&str) -> Result<(), SlackApiError>,
    ) -> Result<(), SlackApiError> {
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.bot_token)
            .json(body)
            .send()
            .await
            .map_err(|e| SlackApiError::Http(e.into()))?;
        read_response(response, interpret).await
    }
}

#[async_trait]
impl SlackApi for ReqwestSlackApi {
    async fn post_message(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<(), SlackApiError> {
        let body = match thread_ts {
            Some(thread_ts) => serde_json::json!({
                "channel": channel, "thread_ts": thread_ts, "text": text
            }),
            None => serde_json::json!({ "channel": channel, "text": text }),
        };
        self.call(POST_MESSAGE_URL, &body, interpret_post_message)
            .await
    }

    async fn add_reaction(&self, channel: &str, ts: &str, name: &str) -> Result<(), SlackApiError> {
        let body = serde_json::json!({ "channel": channel, "timestamp": ts, "name": name });
        self.call(REACTIONS_ADD_URL, &body, interpret_reactions_add)
            .await
    }

    async fn remove_reaction(
        &self,
        channel: &str,
        ts: &str,
        name: &str,
    ) -> Result<(), SlackApiError> {
        let body = serde_json::json!({ "channel": channel, "timestamp": ts, "name": name });
        self.call(REACTIONS_REMOVE_URL, &body, interpret_reactions_remove)
            .await
    }

    async fn open_connection(&self) -> anyhow::Result<String> {
        let response = self
            .http
            .post(CONNECTIONS_OPEN_URL)
            .bearer_auth(&self.app_token)
            .send()
            .await
            .context("apps.connections.open request failed")?;
        let body = response
            .text()
            .await
            .context("apps.connections.open response body read failed")?;
        interpret_open_connection(&body)
    }
}

/// The shared back half of every Web API call: a 429 short-circuits on the
/// header, before any body parsing — the header is authoritative and the
/// body may not be worth reading — and anything else is read as a body and
/// handed to `interpret`.
async fn read_response<T>(
    response: reqwest::Response,
    interpret: fn(&str) -> Result<T, SlackApiError>,
) -> Result<T, SlackApiError> {
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = retry_after_seconds(
            response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
        );
        return Err(SlackApiError::RateLimited { retry_after });
    }

    let body = response
        .text()
        .await
        .map_err(|e| SlackApiError::Http(e.into()))?;
    interpret(&body)
}

/// `Retry-After` per Slack is whole seconds. Missing, unparseable, or zero
/// values floor at one second — never retry immediately against a 429.
fn retry_after_seconds(value: Option<&str>) -> Duration {
    let seconds = value
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(1)
        .max(1);
    Duration::from_secs(seconds)
}

/// Parse a Web API response body: `ok:true` succeeds, `ok:false` is a
/// Slack-side rejection surfaced verbatim — unless its error code is
/// `benign`, an "already in the state you asked for" answer that counts as
/// success — and a non-JSON body means we never reached Slack's API surface.
fn interpret_ok_body(body: &str, method: &str, benign: Option<&str>) -> Result<(), SlackApiError> {
    let body: Value = serde_json::from_str(body)
        .map_err(|e| SlackApiError::Http(anyhow::anyhow!("{method} returned invalid JSON: {e}")))?;
    if body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(());
    }
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown Slack error");
    if benign.is_some_and(|benign| benign == error) {
        return Ok(());
    }
    Err(SlackApiError::Fatal(error.to_string()))
}

/// Interpret a `chat.postMessage` response body.
fn interpret_post_message(body: &str) -> Result<(), SlackApiError> {
    interpret_ok_body(body, "chat.postMessage", None)
}

/// Interpret a `reactions.add` response body; `already_reacted` is success.
fn interpret_reactions_add(body: &str) -> Result<(), SlackApiError> {
    interpret_ok_body(body, "reactions.add", Some("already_reacted"))
}

/// Interpret a `reactions.remove` response body; `no_reaction` is success.
fn interpret_reactions_remove(body: &str) -> Result<(), SlackApiError> {
    interpret_ok_body(body, "reactions.remove", Some("no_reaction"))
}

/// Interpret an `apps.connections.open` response body into the WebSocket URL.
fn interpret_open_connection(body: &str) -> anyhow::Result<String> {
    let body: Value = serde_json::from_str(body)
        .with_context(|| format!("apps.connections.open returned invalid JSON: {body}"))?;
    if !body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        let error = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown Slack error");
        anyhow::bail!("apps.connections.open failed: {error}");
    }
    body.get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("apps.connections.open returned ok without a url"))
}

/// Serializes outbound Web API calls with a minimum interval between them
/// and rides out 429s by honoring Slack's `Retry-After`.
///
/// The mutex is held across the whole exchange — spacing wait, attempt, and
/// any backoff — so concurrent calls queue behind each other instead of
/// racing the limit, on one shared spacing clock. `last` tracks
/// `tokio::time::Instant` (not `std`) because that is the clock
/// [`tokio::time::sleep`] advances, in production and in paused-clock tests
/// alike.
pub(crate) struct RateLimitedSender<A: SlackApi> {
    /// The API calls are made through.
    pub(crate) api: A,
    /// Minimum gap between attempts.
    pub(crate) min_interval: Duration,
    /// Time of the most recent attempt; `None` until the first call.
    pub(crate) last: Mutex<Option<tokio::time::Instant>>,
}

impl<A: SlackApi> RateLimitedSender<A> {
    /// Post `text` to `channel` (in the thread rooted at `thread_ts`, when
    /// given). Rate limiting and error propagation as [`Self::exchange`]
    /// describes.
    ///
    /// # Errors
    /// Propagates the underlying API's `Fatal`/`Http` errors as-is, or an
    /// error stating the rate limit persisted after the bounded retries.
    pub(crate) async fn send(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> anyhow::Result<()> {
        self.exchange(channel, "posting to", || {
            self.api.post_message(channel, thread_ts, text)
        })
        .await
    }

    /// Add the reaction `name` to the message `ts` in `channel`, under the
    /// same spacing and backoff discipline as a post.
    ///
    /// # Errors
    /// Same contract as [`Self::send`].
    pub(crate) async fn react(&self, channel: &str, ts: &str, name: &str) -> anyhow::Result<()> {
        self.exchange(channel, "reacting in", || {
            self.api.add_reaction(channel, ts, name)
        })
        .await
    }

    /// Remove the bot's reaction `name` from the message `ts` in `channel`,
    /// under the same spacing and backoff discipline as a post.
    ///
    /// # Errors
    /// Same contract as [`Self::send`].
    pub(crate) async fn unreact(&self, channel: &str, ts: &str, name: &str) -> anyhow::Result<()> {
        self.exchange(channel, "clearing a reaction in", || {
            self.api.remove_reaction(channel, ts, name)
        })
        .await
    }

    /// One serialized, rate-limited exchange against the Web API — the
    /// shared body of every sender call: first wait out whatever remains of
    /// the minimum interval since the previous attempt, then call the API,
    /// riding out 429s by honoring Slack's `Retry-After`, at most
    /// [`MAX_RATE_LIMIT_RETRIES`] times; sustained limiting surfaces as an
    /// error. `Fatal` and `Http` errors propagate untouched — retrying those
    /// is the caller's policy.
    async fn exchange<T, Fut>(
        &self,
        channel: &str,
        action: &str,
        attempt: impl Fn() -> Fut,
    ) -> anyhow::Result<T>
    where
        Fut: std::future::Future<Output = Result<T, SlackApiError>>,
    {
        let mut last = self.last.lock().await;

        if let Some(previous) = *last {
            let eligible_at = previous + self.min_interval;
            let now = tokio::time::Instant::now();
            if now < eligible_at {
                tokio::time::sleep(eligible_at - now).await;
            }
        }

        let mut rate_limit_retries = 0;
        loop {
            // Every attempt counts against the spacing clock, 429s included —
            // the limit is on requests, not on results.
            *last = Some(tokio::time::Instant::now());

            match attempt().await {
                Ok(value) => return Ok(value),
                Err(SlackApiError::RateLimited { retry_after }) => {
                    if rate_limit_retries >= MAX_RATE_LIMIT_RETRIES {
                        anyhow::bail!(
                            "slack rate limit persisted after {MAX_RATE_LIMIT_RETRIES} retries \
                             (last Retry-After {retry_after:?}) {action} {channel}"
                        );
                    }
                    rate_limit_retries += 1;
                    tracing::warn!(
                        channel,
                        retry_after = ?retry_after,
                        attempt = rate_limit_retries,
                        "slack rate limited, backing off"
                    );
                    tokio::time::sleep(retry_after).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::Arc;

    /// One recorded Web API call, stamped with the tokio clock — which
    /// `start_paused` virtualizes, so recorded times move with simulated time.
    /// `method` is `post`, `react` or `unreact`; `detail` is the post's text
    /// or the reaction's `ts:name`.
    #[derive(Clone, Debug)]
    struct RecordedCall {
        method: &'static str,
        channel: String,
        detail: String,
        at: tokio::time::Instant,
    }

    /// Hand-rolled fake: records every call and replays a scripted sequence
    /// of results, shared across methods. An exhausted (or empty) script
    /// means "succeed".
    #[derive(Clone, Default)]
    struct FakeApi {
        calls: Arc<tokio::sync::Mutex<Vec<RecordedCall>>>,
        script: Arc<tokio::sync::Mutex<VecDeque<Result<(), SlackApiError>>>>,
    }

    impl FakeApi {
        fn scripted(results: Vec<Result<(), SlackApiError>>) -> Self {
            Self {
                script: Arc::new(tokio::sync::Mutex::new(results.into_iter().collect())),
                ..FakeApi::default()
            }
        }

        async fn calls(&self) -> Vec<RecordedCall> {
            self.calls.lock().await.clone()
        }

        async fn record(
            &self,
            method: &'static str,
            channel: &str,
            detail: String,
        ) -> Result<(), SlackApiError> {
            self.calls.lock().await.push(RecordedCall {
                method,
                channel: channel.to_string(),
                detail,
                at: tokio::time::Instant::now(),
            });
            self.script.lock().await.pop_front().unwrap_or(Ok(()))
        }
    }

    #[async_trait::async_trait]
    impl SlackApi for FakeApi {
        async fn post_message(
            &self,
            channel: &str,
            _thread_ts: Option<&str>,
            text: &str,
        ) -> Result<(), SlackApiError> {
            self.record("post", channel, text.to_string()).await
        }

        async fn add_reaction(
            &self,
            channel: &str,
            ts: &str,
            name: &str,
        ) -> Result<(), SlackApiError> {
            self.record("react", channel, format!("{ts}:{name}")).await
        }

        async fn remove_reaction(
            &self,
            channel: &str,
            ts: &str,
            name: &str,
        ) -> Result<(), SlackApiError> {
            self.record("unreact", channel, format!("{ts}:{name}"))
                .await
        }

        async fn open_connection(&self) -> anyhow::Result<String> {
            Ok("wss://fake.slack".to_string())
        }
    }

    fn sender(api: &FakeApi, min_interval_ms: u64) -> RateLimitedSender<FakeApi> {
        RateLimitedSender {
            api: api.clone(),
            min_interval: std::time::Duration::from_millis(min_interval_ms),
            last: tokio::sync::Mutex::new(None),
        }
    }

    /// Gaps between consecutive recorded calls, in recorded order.
    fn gaps(calls: &[RecordedCall]) -> Vec<std::time::Duration> {
        calls
            .iter()
            .zip(calls.iter().skip(1))
            .map(|(earlier, later)| later.at - earlier.at)
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn sender_enforces_min_interval() {
        let api = FakeApi::default();
        let sender = sender(&api, 1000);

        sender.send("D1", None, "one").await.unwrap();
        sender.send("D1", None, "two").await.unwrap();

        let calls = api.calls().await;
        assert_eq!(calls.len(), 2);
        let texts: Vec<&str> = calls.iter().map(|call| call.detail.as_str()).collect();
        assert_eq!(texts, ["one", "two"]);
        assert!(calls.iter().all(|call| call.channel == "D1"));
        for gap in gaps(&calls) {
            assert!(
                gap >= std::time::Duration::from_millis(1000),
                "the second send must wait out the minimum interval"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn sender_retries_after_rate_limit_honoring_retry_after() {
        let api = FakeApi::scripted(vec![
            Err(SlackApiError::RateLimited {
                retry_after: std::time::Duration::from_secs(2),
            }),
            Ok(()),
        ]);
        let sender = sender(&api, 100);

        sender
            .send("D1", None, "one")
            .await
            .expect("a single rate limit must be retried, not surfaced");

        let calls = api.calls().await;
        assert_eq!(calls.len(), 2, "exactly one retry attempt");
        for gap in gaps(&calls) {
            assert!(
                gap >= std::time::Duration::from_secs(2),
                "the retry must wait out the full Retry-After"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn sender_propagates_fatal_without_retry() {
        let api = FakeApi::scripted(vec![Err(SlackApiError::Fatal(
            "channel_not_found".to_string(),
        ))]);
        let sender = sender(&api, 100);

        let err = sender.send("D1", None, "one").await.unwrap_err();
        assert!(
            err.to_string().contains("channel_not_found"),
            "fatal slack errors must surface to the caller: {err}"
        );
        assert_eq!(api.calls().await.len(), 1, "fatal errors are not retried");
    }

    #[tokio::test(start_paused = true)]
    async fn sender_surfaces_sustained_rate_limiting() {
        let rate_limited = || SlackApiError::RateLimited {
            retry_after: std::time::Duration::from_millis(50),
        };
        let api = FakeApi::scripted(vec![
            Err(rate_limited()),
            Err(rate_limited()),
            Err(rate_limited()),
            Err(rate_limited()),
            Err(rate_limited()),
        ]);
        let sender = sender(&api, 100);

        let err = sender.send("D1", None, "one").await.unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("rate limit"),
            "sustained 429s must surface as errors, not throttle silently: {err}"
        );
        assert_eq!(
            api.calls().await.len(),
            1 + MAX_RATE_LIMIT_RETRIES,
            "retries are bounded"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_sends_are_serialized_with_min_interval() {
        let api = FakeApi::default();
        let sender = Arc::new(sender(&api, 1000));

        let mut handles = Vec::new();
        for i in 0..3 {
            let sender = Arc::clone(&sender);
            handles.push(tokio::spawn(async move {
                sender.send("D1", None, &format!("msg-{i}")).await.unwrap();
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }

        let mut calls = api.calls().await;
        assert_eq!(calls.len(), 3);
        calls.sort_by_key(|call| call.at);
        for gap in gaps(&calls) {
            assert!(
                gap >= std::time::Duration::from_millis(1000),
                "every send must be spaced by the minimum interval: {calls:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reactions_ride_the_same_spacing_and_clock_as_posts() {
        let api = FakeApi::default();
        let sender = sender(&api, 1000);

        sender.react("D1", "1.1", "eyes").await.unwrap();
        sender.send("D1", None, "reply").await.unwrap();
        sender.unreact("D1", "1.1", "eyes").await.unwrap();

        let calls = api.calls().await;
        let methods: Vec<&str> = calls.iter().map(|call| call.method).collect();
        assert_eq!(methods, ["react", "post", "unreact"]);
        assert_eq!(calls.first().unwrap().detail, "1.1:eyes");
        assert_eq!(calls.last().unwrap().detail, "1.1:eyes");
        for gap in gaps(&calls) {
            assert!(
                gap >= std::time::Duration::from_millis(1000),
                "reactions share the posts' minimum interval: {calls:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_reaction_backs_off_and_retries() {
        let api = FakeApi::scripted(vec![Err(SlackApiError::RateLimited {
            retry_after: std::time::Duration::from_secs(2),
        })]);
        let sender = sender(&api, 100);

        sender
            .unreact("D1", "1.1", "eyes")
            .await
            .expect("a single rate limit must be retried, not surfaced");

        let calls = api.calls().await;
        assert_eq!(calls.len(), 2, "exactly one retry attempt");
        assert!(
            gaps(&calls)
                .iter()
                .all(|gap| *gap >= std::time::Duration::from_secs(2)),
            "the retry must wait out the full Retry-After"
        );
    }

    #[test]
    fn retry_after_seconds_parses_whole_seconds() {
        assert_eq!(
            retry_after_seconds(Some("3")),
            std::time::Duration::from_secs(3)
        );
    }

    #[test]
    fn retry_after_seconds_falls_back_to_one_second() {
        let one_second = std::time::Duration::from_secs(1);
        assert_eq!(retry_after_seconds(None), one_second, "missing header");
        assert_eq!(retry_after_seconds(Some("soon")), one_second, "unparseable");
        assert_eq!(
            retry_after_seconds(Some("0")),
            one_second,
            "never a hot loop"
        );
    }

    #[test]
    fn post_message_ok_body_succeeds() {
        interpret_post_message(r#"{"ok":true,"ts":"1728558000.000100"}"#)
            .expect("ok:true is a successful post");
    }

    #[test]
    fn post_message_error_body_is_fatal() {
        let err =
            interpret_post_message(r#"{"ok":false,"error":"channel_not_found"}"#).unwrap_err();
        match err {
            SlackApiError::Fatal(msg) => assert_eq!(msg, "channel_not_found"),
            other @ (SlackApiError::Http(_) | SlackApiError::RateLimited { .. }) => {
                panic!("expected Fatal, got {other:?}")
            }
        }
    }

    #[test]
    fn post_message_error_body_without_error_field_is_fatal_placeholder() {
        let err = interpret_post_message(r#"{"ok":false}"#).unwrap_err();
        match err {
            SlackApiError::Fatal(msg) => assert_eq!(msg, "unknown Slack error"),
            other @ (SlackApiError::Http(_) | SlackApiError::RateLimited { .. }) => {
                panic!("expected Fatal, got {other:?}")
            }
        }
    }

    #[test]
    fn post_message_garbage_body_is_http() {
        let err = interpret_post_message("<html>gateway 502</html>").unwrap_err();
        assert!(
            matches!(err, SlackApiError::Http(_)),
            "a non-JSON body is a transport-level problem: {err:?}"
        );
    }

    #[test]
    fn already_reacted_counts_as_success() {
        interpret_reactions_add(r#"{"ok":false,"error":"already_reacted"}"#)
            .expect("the reaction is there, which is all add wanted");
    }

    #[test]
    fn no_reaction_counts_as_success_on_remove() {
        interpret_reactions_remove(r#"{"ok":false,"error":"no_reaction"}"#)
            .expect("the reaction is gone, which is all remove wanted");
    }

    #[test]
    fn a_missing_scope_on_a_reaction_is_fatal() {
        let err = interpret_reactions_add(r#"{"ok":false,"error":"missing_scope"}"#).unwrap_err();
        assert!(
            matches!(&err, SlackApiError::Fatal(msg) if msg == "missing_scope"),
            "a reaction without reactions:write must surface: {err:?}"
        );
        // The benign code of one method is not benign for the other.
        assert!(
            interpret_reactions_remove(r#"{"ok":false,"error":"already_reacted"}"#).is_err(),
            "already_reacted is only benign for an add"
        );
    }

    #[test]
    fn open_connection_ok_returns_url() {
        let url = interpret_open_connection(r#"{"ok":true,"url":"wss://wss-primary.slack.com"}"#)
            .expect("ok:true with a url is a success");
        assert_eq!(url, "wss://wss-primary.slack.com");
    }

    #[test]
    fn open_connection_error_body_fails_with_error_code() {
        let err =
            interpret_open_connection(r#"{"ok":false,"error":"invalid_app_token"}"#).unwrap_err();
        assert!(
            err.to_string().contains("invalid_app_token"),
            "the Slack error code must surface: {err}"
        );
    }

    #[test]
    fn open_connection_ok_without_url_fails() {
        let err = interpret_open_connection(r#"{"ok":true}"#).unwrap_err();
        assert!(
            err.to_string().contains("url"),
            "ok:true missing its url is unusable: {err}"
        );
    }
}
