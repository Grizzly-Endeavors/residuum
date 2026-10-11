//! Web API client for Slack plus the rate-limited sender that wraps it.
//!
//! Thin by design (design doc, module table): `chat.postMessage` behind the
//! [`SlackApi`] trait, and the minimum-interval sender that serializes posts.
//! Knows nothing about sockets or conversations — [`super::events`] and the
//! socket task own those.

use anyhow::Context as _;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::Mutex;

/// `chat.postMessage` — the only write the interface makes.
const POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";
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
    /// Post `text` to `channel` via `chat.postMessage`.
    ///
    /// # Errors
    /// [`SlackApiError::RateLimited`] on HTTP 429 — the caller is expected to
    /// honor `retry_after`; `Fatal` for Slack-side `ok:false` rejections;
    /// `Http` for transport failures.
    async fn post_message(&self, channel: &str, text: &str) -> Result<(), SlackApiError>;

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
    async fn post_message(&self, channel: &str, text: &str) -> Result<(), SlackApiError> {
        let response = self
            .http
            .post(POST_MESSAGE_URL)
            .bearer_auth(&self.bot_token)
            .json(&serde_json::json!({ "channel": channel, "text": text }))
            .send()
            .await
            .map_err(|e| SlackApiError::Http(e.into()))?;

        // 429 short-circuits on the header, before any body parsing: the
        // header is authoritative and the body may not be worth reading.
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
        interpret_post_message(&body)
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

/// `Retry-After` per Slack is whole seconds. Missing, unparseable, or zero
/// values floor at one second — never retry immediately against a 429.
fn retry_after_seconds(value: Option<&str>) -> Duration {
    let seconds = value
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(1)
        .max(1);
    Duration::from_secs(seconds)
}

/// Interpret a `chat.postMessage` response body: `ok:true` succeeds,
/// `ok:false` is a Slack-side rejection surfaced verbatim, and a non-JSON
/// body means we never reached Slack's API surface.
fn interpret_post_message(body: &str) -> Result<(), SlackApiError> {
    let body: Value = serde_json::from_str(body).map_err(|e| {
        SlackApiError::Http(anyhow::anyhow!(
            "chat.postMessage returned invalid JSON: {e}"
        ))
    })?;
    if body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(());
    }
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown Slack error")
        .to_string();
    Err(SlackApiError::Fatal(error))
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

/// Serializes outbound posts with a minimum interval between them and rides
/// out 429s by honoring Slack's `Retry-After`.
///
/// The mutex is held across the whole exchange — spacing wait, attempt, and
/// any backoff — so concurrent `send`s queue behind each other instead of
/// racing the limit. `last` tracks `tokio::time::Instant` (not `std`) because
/// that is the clock [`tokio::time::sleep`] advances, in production and in
/// paused-clock tests alike.
pub(crate) struct RateLimitedSender<A: SlackApi> {
    /// The API calls are made through.
    pub(crate) api: A,
    /// Minimum gap between `post_message` attempts.
    pub(crate) min_interval: Duration,
    /// Time of the most recent attempt; `None` until the first send.
    pub(crate) last: Mutex<Option<tokio::time::Instant>>,
}

impl<A: SlackApi> RateLimitedSender<A> {
    /// Post `text` to `channel`, first waiting out whatever remains of the
    /// minimum interval since the previous attempt. A 429 backs off for its
    /// `Retry-After` and retries, at most [`MAX_RATE_LIMIT_RETRIES`] times;
    /// sustained limiting surfaces as an error. `Fatal` and `Http` errors
    /// propagate untouched — retrying those is the caller's policy.
    ///
    /// # Errors
    /// Propagates the underlying API's `Fatal`/`Http` errors as-is, or an
    /// error stating the rate limit persisted after the bounded retries.
    pub(crate) async fn send(&self, channel: &str, text: &str) -> anyhow::Result<()> {
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

            match self.api.post_message(channel, text).await {
                Ok(()) => return Ok(()),
                Err(SlackApiError::RateLimited { retry_after }) => {
                    if rate_limit_retries >= MAX_RATE_LIMIT_RETRIES {
                        anyhow::bail!(
                            "slack rate limit persisted after {MAX_RATE_LIMIT_RETRIES} retries \
                             (last Retry-After {retry_after:?}) posting to {channel}"
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

    /// Hand-rolled fake: records every call and replays a scripted sequence
    /// of results. An exhausted (or empty) script means "succeed".
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
    }

    #[async_trait::async_trait]
    impl SlackApi for FakeApi {
        async fn post_message(&self, channel: &str, text: &str) -> Result<(), SlackApiError> {
            self.calls.lock().await.push(RecordedCall {
                channel: channel.to_string(),
                text: text.to_string(),
                at: tokio::time::Instant::now(),
            });
            self.script.lock().await.pop_front().unwrap_or(Ok(()))
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
            Ok(()),
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
        interpret_post_message(r#"{"ok":true}"#).expect("ok:true is a successful post");
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
