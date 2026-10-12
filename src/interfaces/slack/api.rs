//! Web API client for Slack plus the rate-limited sender that wraps it.
//!
//! Thin by design (design doc, module table): `chat.postMessage` and
//! `chat.update` behind the [`SlackApi`] trait, and the minimum-interval
//! sender that serializes both. Knows nothing about sockets or
//! conversations — [`super::events`] and the socket task own those.

use anyhow::Context as _;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::Mutex;

/// `chat.postMessage` — posts a fresh message, returning its `ts`.
const POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";
/// `chat.update` — edits a previously posted message in place, by `ts`. The
/// placeholder pattern's whole editing surface; rides the same `chat:write`
/// scope `chat.postMessage` already has.
const CHAT_UPDATE_URL: &str = "https://slack.com/api/chat.update";
/// `apps.connections.open` — Socket Mode handshake, returns the WebSocket URL.
const CONNECTIONS_OPEN_URL: &str = "https://slack.com/api/apps.connections.open";

/// Per-request timeout for Slack Web API calls, matching the Teams adapter's
/// budget for its Microsoft-side HTTP.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Most 429 backoff rounds one `send` rides out before surfacing the rate
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
    /// Post `text` to `channel` via `chat.postMessage`, returning the posted
    /// message's `ts` — the handle a later [`Self::chat_update`] edits.
    ///
    /// # Errors
    /// [`SlackApiError::RateLimited`] on HTTP 429 — the caller is expected to
    /// honor `retry_after`; `Fatal` for Slack-side `ok:false` rejections;
    /// `Http` for transport failures.
    async fn post_message(&self, channel: &str, text: &str) -> Result<String, SlackApiError>;

    /// Edit the message `ts` in `channel` to `text` via `chat.update`.
    /// Errors are classified exactly like [`Self::post_message`]'s.
    ///
    /// # Errors
    /// Same contract as [`Self::post_message`].
    async fn chat_update(&self, channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError>;

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
}

#[async_trait]
impl SlackApi for ReqwestSlackApi {
    async fn post_message(&self, channel: &str, text: &str) -> Result<String, SlackApiError> {
        let response = self
            .http
            .post(POST_MESSAGE_URL)
            .bearer_auth(&self.bot_token)
            .json(&serde_json::json!({ "channel": channel, "text": text }))
            .send()
            .await
            .map_err(|e| SlackApiError::Http(e.into()))?;

        read_response(response, interpret_post_message).await
    }

    async fn chat_update(&self, channel: &str, ts: &str, text: &str) -> Result<(), SlackApiError> {
        let response = self
            .http
            .post(CHAT_UPDATE_URL)
            .bearer_auth(&self.bot_token)
            .json(&serde_json::json!({ "channel": channel, "ts": ts, "text": text }))
            .send()
            .await
            .map_err(|e| SlackApiError::Http(e.into()))?;

        read_response(response, interpret_chat_update).await
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

/// Parse a `chat.*` response body: `ok:true` yields the parsed body,
/// `ok:false` is a Slack-side rejection surfaced verbatim, and a non-JSON
/// body means we never reached Slack's API surface.
fn interpret_ok_body(body: &str, method: &str) -> Result<Value, SlackApiError> {
    let body: Value = serde_json::from_str(body)
        .map_err(|e| SlackApiError::Http(anyhow::anyhow!("{method} returned invalid JSON: {e}")))?;
    if body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(body);
    }
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown Slack error")
        .to_string();
    Err(SlackApiError::Fatal(error))
}

/// Interpret a `chat.postMessage` response body into the posted message's
/// `ts` — the handle the placeholder pattern later edits.
fn interpret_post_message(body: &str) -> Result<String, SlackApiError> {
    let body = interpret_ok_body(body, "chat.postMessage")?;
    body.get("ts")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            SlackApiError::Http(anyhow::anyhow!("chat.postMessage returned ok without a ts"))
        })
}

/// Interpret a `chat.update` response body: `ok:true` succeeds, anything
/// else is classified like a post.
fn interpret_chat_update(body: &str) -> Result<(), SlackApiError> {
    interpret_ok_body(body, "chat.update").map(|_| ())
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

/// Serializes outbound posts and edits with a minimum interval between them
/// and rides out 429s by honoring Slack's `Retry-After`.
///
/// The mutex is held across the whole exchange — spacing wait, attempt, and
/// any backoff — so concurrent `send`s and `edit`s queue behind each other
/// instead of racing the limit, on one shared spacing clock. `last` tracks
/// `tokio::time::Instant` (not `std`) because that is the clock
/// [`tokio::time::sleep`] advances, in production and in paused-clock tests
/// alike.
pub(crate) struct RateLimitedSender<A: SlackApi> {
    /// The API calls are made through.
    pub(crate) api: A,
    /// Minimum gap between attempts.
    pub(crate) min_interval: Duration,
    /// Time of the most recent attempt; `None` until the first send.
    pub(crate) last: Mutex<Option<tokio::time::Instant>>,
}

impl<A: SlackApi> RateLimitedSender<A> {
    /// Post `text` to `channel` via `chat.postMessage`, returning the posted
    /// message's `ts`. Rate limiting and error propagation as
    /// [`Self::exchange`] describes.
    ///
    /// # Errors
    /// Propagates the underlying API's `Fatal`/`Http` errors as-is, or an
    /// error stating the rate limit persisted after the bounded retries.
    pub(crate) async fn send(&self, channel: &str, text: &str) -> anyhow::Result<String> {
        self.exchange(channel, "posting to", || {
            self.api.post_message(channel, text)
        })
        .await
    }

    /// Edit the message `ts` in `channel` to `text` via `chat.update`, under
    /// the same spacing and backoff discipline as a post.
    ///
    /// # Errors
    /// Same contract as [`Self::send`].
    pub(crate) async fn edit(&self, channel: &str, ts: &str, text: &str) -> anyhow::Result<()> {
        self.exchange(channel, "editing in", || {
            self.api.chat_update(channel, ts, text)
        })
        .await
    }

    /// One serialized, rate-limited exchange against the Web API — the
    /// shared body of [`Self::send`] and [`Self::edit`]: first wait out
    /// whatever remains of the minimum interval since the previous attempt,
    /// then call the API, riding out 429s by honoring Slack's `Retry-After`,
    /// at most [`MAX_RATE_LIMIT_RETRIES`] times; sustained limiting surfaces
    /// as an error. `Fatal` and `Http` errors propagate untouched — retrying
    /// those is the caller's policy.
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

    /// One recorded `post_message` call, stamped with the tokio clock — which
    /// `start_paused` virtualizes, so recorded times move with simulated time.
    #[derive(Clone, Debug)]
    struct RecordedCall {
        channel: String,
        text: String,
        at: tokio::time::Instant,
    }

    /// One recorded `chat_update` call, on the same virtualized clock.
    #[derive(Clone, Debug)]
    struct RecordedUpdate {
        channel: String,
        ts: String,
        text: String,
        at: tokio::time::Instant,
    }

    /// Hand-rolled fake: records every call and replays a scripted sequence
    /// of results. An exhausted (or empty) script means "succeed"; a post
    /// "succeeds" with a synthetic ts derived from the call count, so a test
    /// can feed it straight back into an edit.
    #[derive(Clone, Default)]
    struct FakeApi {
        calls: Arc<tokio::sync::Mutex<Vec<RecordedCall>>>,
        updates: Arc<tokio::sync::Mutex<Vec<RecordedUpdate>>>,
        script: Arc<tokio::sync::Mutex<VecDeque<Result<String, SlackApiError>>>>,
        update_script: Arc<tokio::sync::Mutex<VecDeque<Result<(), SlackApiError>>>>,
    }

    impl FakeApi {
        fn scripted(results: Vec<Result<String, SlackApiError>>) -> Self {
            Self {
                script: Arc::new(tokio::sync::Mutex::new(results.into_iter().collect())),
                ..FakeApi::default()
            }
        }

        async fn calls(&self) -> Vec<RecordedCall> {
            self.calls.lock().await.clone()
        }

        async fn updates(&self) -> Vec<RecordedUpdate> {
            self.updates.lock().await.clone()
        }
    }

    #[async_trait::async_trait]
    impl SlackApi for FakeApi {
        async fn post_message(&self, channel: &str, text: &str) -> Result<String, SlackApiError> {
            let ts = format!("ts.{}", self.calls.lock().await.len() + 1);
            self.calls.lock().await.push(RecordedCall {
                channel: channel.to_string(),
                text: text.to_string(),
                at: tokio::time::Instant::now(),
            });
            self.script.lock().await.pop_front().unwrap_or(Ok(ts))
        }

        async fn chat_update(
            &self,
            channel: &str,
            ts: &str,
            text: &str,
        ) -> Result<(), SlackApiError> {
            self.updates.lock().await.push(RecordedUpdate {
                channel: channel.to_string(),
                ts: ts.to_string(),
                text: text.to_string(),
                at: tokio::time::Instant::now(),
            });
            self.update_script
                .lock()
                .await
                .pop_front()
                .unwrap_or(Ok(()))
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

        sender.send("D1", "one").await.unwrap();
        sender.send("D1", "two").await.unwrap();

        let calls = api.calls().await;
        assert_eq!(calls.len(), 2);
        let texts: Vec<&str> = calls.iter().map(|call| call.text.as_str()).collect();
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
            Ok("ts.2".to_string()),
        ]);
        let sender = sender(&api, 100);

        sender
            .send("D1", "one")
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

        let err = sender.send("D1", "one").await.unwrap_err();
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

        let err = sender.send("D1", "one").await.unwrap_err();
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
                sender.send("D1", &format!("msg-{i}")).await.unwrap();
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
    async fn edits_ride_the_same_spacing_and_clock_as_posts() {
        let api = FakeApi::default();
        let sender = sender(&api, 1000);

        let ts = sender.send("D1", "one").await.unwrap();
        sender.edit("D1", &ts, "one (edited)").await.unwrap();

        let calls = api.calls().await;
        let updates = api.updates().await;
        assert_eq!(calls.len(), 1, "one post: the original");
        assert_eq!(updates.len(), 1, "one edit: of the posted ts");
        let call = calls.first().expect("the post was recorded");
        let update = updates.first().expect("the edit was recorded");
        assert_eq!(update.channel, call.channel);
        assert_eq!(update.ts, ts, "the edit names the post's ts");
        assert_eq!(update.text, "one (edited)");
        assert!(
            update.at - call.at >= std::time::Duration::from_millis(1000),
            "an edit waits out the same minimum interval as a post"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_edit_backs_off_and_retries() {
        let api = FakeApi::default();
        {
            let mut script = api.update_script.lock().await;
            script.push_back(Err(SlackApiError::RateLimited {
                retry_after: std::time::Duration::from_secs(2),
            }));
        }
        let sender = sender(&api, 100);

        sender
            .edit("D1", "ts.9", "fixed")
            .await
            .expect("a single rate limit must be retried, not surfaced");

        let updates = api.updates().await;
        assert_eq!(updates.len(), 2, "exactly one retry attempt");
        let first = updates.first().expect("the first attempt was recorded");
        let retry = updates.get(1).expect("the retried attempt was recorded");
        assert!(
            retry.at - first.at >= std::time::Duration::from_secs(2),
            "the retry must wait out the full Retry-After"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn edits_propagate_fatal_without_retry() {
        let api = FakeApi::default();
        {
            let mut script = api.update_script.lock().await;
            script.push_back(Err(SlackApiError::Fatal("message_not_found".to_string())));
        }
        let sender = sender(&api, 100);

        let err = sender.edit("D1", "ts.9", "fixed").await.unwrap_err();
        assert!(
            err.to_string().contains("message_not_found"),
            "fatal slack errors must surface to the caller: {err}"
        );
        assert_eq!(api.updates().await.len(), 1, "fatal errors are not retried");
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
    fn post_message_ok_body_returns_its_ts() {
        assert_eq!(
            interpret_post_message(r#"{"ok":true,"ts":"1728558000.000100"}"#)
                .expect("ok:true with a ts is a successful post"),
            "1728558000.000100",
            "the ts is what chat.update needs, so the post must surface it"
        );
    }

    #[test]
    fn post_message_ok_body_without_ts_is_http() {
        let err = interpret_post_message(r#"{"ok":true}"#).unwrap_err();
        assert!(
            matches!(err, SlackApiError::Http(_)),
            "ok:true missing its ts is unusable for the placeholder edit: {err:?}"
        );
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
    fn chat_update_ok_body_succeeds() {
        interpret_chat_update(r#"{"ok":true,"ts":"1728558000.000100"}"#)
            .expect("ok:true is a successful edit");
    }

    #[test]
    fn chat_update_error_body_is_fatal() {
        let err = interpret_chat_update(r#"{"ok":false,"error":"message_not_found"}"#).unwrap_err();
        match err {
            SlackApiError::Fatal(msg) => assert_eq!(msg, "message_not_found"),
            other @ (SlackApiError::Http(_) | SlackApiError::RateLimited { .. }) => {
                panic!("expected Fatal, got {other:?}")
            }
        }
    }

    #[test]
    fn chat_update_garbage_body_is_http() {
        let err = interpret_chat_update("<html>gateway 502</html>").unwrap_err();
        assert!(
            matches!(err, SlackApiError::Http(_)),
            "a non-JSON body is a transport-level problem: {err:?}"
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
