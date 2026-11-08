//! Shared HTTP utilities for model provider clients.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;

use super::InferenceError;

/// Configuration for HTTP client connection pooling.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Request timeout in seconds. Bounds a whole request/response exchange;
    /// for a streaming response it bounds how long the stream may go without
    /// delivering a byte instead, so a long answer is never cut off for its
    /// length.
    pub timeout_secs: u64,
    /// Maximum idle connections per host (default: 10).
    pub(crate) pool_max_idle_per_host: usize,
    /// HTTP/2 keep-alive interval in seconds (default: 30).
    pub(crate) http2_keep_alive_secs: u64,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 60,
            pool_max_idle_per_host: 10,
            http2_keep_alive_secs: 30,
        }
    }
}

impl HttpClientConfig {
    /// Create a config with the specified timeout and default pool settings.
    #[must_use]
    pub fn with_timeout(timeout_secs: u64) -> Self {
        Self {
            timeout_secs,
            ..Self::default()
        }
    }
}

/// Shared HTTP client wrapper for connection reuse across model providers.
///
/// Uses `Arc` internally, making `Clone` cheap and allowing multiple
/// providers to share the same underlying connection pool.
#[derive(Clone)]
pub struct SharedHttpClient {
    client: Arc<Client>,
    stream_client: Arc<Client>,
    timeout_secs: u64,
}

impl SharedHttpClient {
    /// Create a new shared HTTP client with the specified configuration.
    ///
    /// # Errors
    /// Returns `InferenceError::Request` if the HTTP client cannot be built.
    pub fn new(config: &HttpClientConfig) -> Result<Self, InferenceError> {
        let pooled = || {
            Client::builder()
                .pool_max_idle_per_host(config.pool_max_idle_per_host)
                .http2_keep_alive_interval(Duration::from_secs(config.http2_keep_alive_secs))
        };
        let timeout = Duration::from_secs(config.timeout_secs);
        // A streaming answer can legitimately run for minutes, so it gets a
        // read timeout (reset by every byte, and covering the wait for the
        // response to begin) where a whole-request answer gets a total one.
        let client = pooled().timeout(timeout).build()?;
        let stream_client = pooled().read_timeout(timeout).build()?;

        Ok(Self {
            client: Arc::new(client),
            stream_client: Arc::new(stream_client),
            timeout_secs: config.timeout_secs,
        })
    }

    /// Get a reference to the underlying HTTP client, whose requests time
    /// out as a whole after the configured timeout.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Get the HTTP client for streaming requests. It has no total timeout:
    /// a request fails only when no bytes arrive for the configured timeout.
    #[must_use]
    pub(crate) fn streaming_client(&self) -> &Client {
        &self.stream_client
    }

    /// Get the configured timeout in seconds.
    #[must_use]
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }
}

/// Check if a URL is using insecure HTTP for a remote (non-localhost) server.
fn is_insecure_remote_url(url: &str) -> bool {
    let url_lower = url.to_lowercase();
    url_lower.starts_with("http://")
        && !url_lower.contains("localhost")
        && !url_lower.contains("127.0.0.1")
        && !url_lower.contains("[::1]")
}

/// Warn if a URL uses insecure HTTP for a remote server.
pub fn warn_if_insecure_remote(url: &str) {
    if is_insecure_remote_url(url) {
        tracing::warn!(
            url = %url,
            "using unencrypted HTTP for non-localhost API; consider using HTTPS"
        );
    }
}

/// Map a reqwest error to a [`InferenceError`], detecting timeouts.
///
/// The URL is dropped from the error before it is wrapped: reqwest's text
/// includes the URL, and a provider may carry a credential in one.
pub fn map_request_error(e: reqwest::Error, timeout_secs: u64) -> InferenceError {
    if e.is_timeout() {
        InferenceError::Timeout(timeout_secs)
    } else {
        InferenceError::Request(e.without_url())
    }
}

/// Map an error from sending a streaming request to a [`InferenceError`].
/// Drops the URL as [`map_request_error`] does.
pub fn map_stream_request_error(e: reqwest::Error, idle_secs: u64) -> InferenceError {
    if e.is_timeout() {
        InferenceError::Stalled(idle_secs)
    } else {
        InferenceError::Request(e.without_url())
    }
}

/// Map an error from reading a streaming response body: the model going
/// quiet is a stall, anything else (a dropped connection, a body that ends
/// mid-chunk) an interrupted stream. Drops the URL as [`map_request_error`] does.
pub fn map_stream_read_error(e: reqwest::Error, idle_secs: u64) -> InferenceError {
    if e.is_timeout() {
        InferenceError::Stalled(idle_secs)
    } else {
        InferenceError::StreamInterrupted(error_chain(&e.without_url()))
    }
}

/// An error and its sources on one line, outermost first.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Read the body of an error response, falling back to a placeholder if reading fails.
pub async fn read_error_body(response: reqwest::Response) -> String {
    match response.text().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read error response body");
            format!("failed to read response body: {e}")
        }
    }
}

/// Whether an HTTP status code represents a transient failure (timeout, rate
/// limit, Groq capacity refusal, or 5xx server error) that will be retried.
#[must_use]
pub(crate) const fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 498) || status >= 500
}

/// Log a non-2xx provider HTTP response without dumping full request prompts
/// and observation history at `WARN`.
///
/// - `WARN` always records `provider`, `status`, `request_bytes`, and
///   `response_body`.
/// - For retryable transient statuses (`408`, `429`, `498`, `5xx`), the
///   media-redacted `request_body` is logged at `TRACE`.
/// - For non-retryable client errors (`400`, `404`, `422`, etc., where the
///   request payload itself was rejected), the media-redacted `request_body`
///   is logged at `DEBUG`.
pub(crate) fn log_api_error(
    provider: &str,
    status: reqwest::StatusCode,
    response_body: &str,
    request_json: &str,
) {
    tracing::warn!(
        provider = %provider,
        status = %status,
        request_bytes = request_json.len(),
        response_body = %response_body,
        "provider API error"
    );
    if is_retryable_status(status.as_u16()) {
        if tracing::enabled!(tracing::Level::TRACE) {
            tracing::trace!(
                provider = %provider,
                status = %status,
                request_body = %without_inline_media(request_json),
                "provider API error request payload"
            );
        }
    } else if tracing::enabled!(tracing::Level::DEBUG) {
        tracing::debug!(
            provider = %provider,
            status = %status,
            request_body = %without_inline_media(request_json),
            "provider API error request payload"
        );
    }
}

/// Shortest inline media payload [`without_inline_media`] replaces. Shorter
/// strings are left alone: a base64-looking word in a message is kept.
const INLINE_MEDIA_MIN_LEN: usize = 256;

/// A request body as JSON text for a diagnostic log, with every inline media
/// payload (a base64 image, or a `data:` URL holding one) replaced by its
/// size: a failed request with images would otherwise write megabytes of
/// base64 into the log each time it is refused. Text that isn't JSON comes
/// back unchanged.
pub(crate) fn without_inline_media(request_json: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(request_json) else {
        return request_json.to_string();
    };
    replace_inline_media(&mut value);
    value.to_string()
}

fn replace_inline_media(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) if is_inline_media(text) => {
            *text = format!("[{} bytes of inline media omitted]", text.len());
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(replace_inline_media),
        serde_json::Value::Object(fields) => fields.values_mut().for_each(replace_inline_media),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

fn is_inline_media(text: &str) -> bool {
    if text.len() < INLINE_MEDIA_MIN_LEN {
        return false;
    }
    let payload = match text.strip_prefix("data:") {
        Some(url) => match url.split_once(";base64,") {
            Some((_, data)) => data,
            None => return false,
        },
        None => text,
    };
    payload
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'\n' | b'\r'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ScriptedServer, Step};

    #[test]
    fn logged_requests_leave_out_inline_media() {
        let image = "iVBORw0KGgo".repeat(100);
        let request = serde_json::json!({
            "model": "glm-5.3",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "what is this?"},
                    {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{image}")}},
                ],
            }, {
                "role": "user",
                "content": [{"type": "image", "source": {"type": "base64", "data": image}}],
            }],
        })
        .to_string();

        let logged = without_inline_media(&request);

        assert!(
            !logged.contains(&image),
            "no image payload is logged: {logged}"
        );
        assert!(logged.contains("what is this?"), "text is kept: {logged}");
        assert!(logged.contains("bytes of inline media omitted"), "{logged}");
        assert_eq!(without_inline_media("not json"), "not json");
    }

    #[test]
    fn insecure_url_detection_remote() {
        assert!(
            is_insecure_remote_url("http://api.example.com:11434"),
            "remote HTTP should be insecure"
        );
        assert!(
            is_insecure_remote_url("http://192.168.1.1:11434"),
            "private IP over HTTP should be insecure"
        );
    }

    #[test]
    fn insecure_url_detection_local() {
        assert!(
            !is_insecure_remote_url("http://localhost:11434"),
            "localhost HTTP is acceptable"
        );
        assert!(
            !is_insecure_remote_url("http://127.0.0.1:11434"),
            "loopback HTTP is acceptable"
        );
        assert!(
            !is_insecure_remote_url("http://[::1]:11434"),
            "IPv6 loopback HTTP is acceptable"
        );
    }

    #[test]
    fn insecure_url_detection_https() {
        assert!(
            !is_insecure_remote_url("https://api.example.com"),
            "HTTPS is always acceptable"
        );
    }

    #[test]
    fn http_client_config_default() {
        let config = HttpClientConfig::default();
        assert_eq!(config.timeout_secs, 60, "default timeout should be 60s");
        assert_eq!(
            config.pool_max_idle_per_host, 10,
            "default pool size should be 10"
        );
        assert_eq!(
            config.http2_keep_alive_secs, 30,
            "default keep-alive should be 30s"
        );
    }

    #[test]
    fn http_client_config_with_timeout() {
        let config = HttpClientConfig::with_timeout(120);
        assert_eq!(config.timeout_secs, 120, "timeout should match requested");
        assert_eq!(
            config.pool_max_idle_per_host, 10,
            "pool size should be default"
        );
    }

    #[test]
    fn shared_http_client_builds() {
        let config = HttpClientConfig::with_timeout(45);
        let shared = SharedHttpClient::new(&config).unwrap();
        assert_eq!(shared.timeout_secs(), 45, "timeout should be stored");
        assert!(
            shared.client().get("http://localhost").build().is_ok(),
            "client should be usable"
        );
    }

    #[test]
    fn shared_http_client_clone_is_cheap() {
        let config = HttpClientConfig::default();
        let client1 = SharedHttpClient::new(&config).unwrap();
        let client2 = client1.clone();
        assert!(
            Arc::ptr_eq(&client1.client, &client2.client),
            "clones should share underlying Arc"
        );
    }

    /// A response that sends a chunk every 100ms for two seconds, twice the
    /// one-second timeout it streams under.
    fn slow_steady_response() -> Vec<Step> {
        let mut script = vec![Step::head(200, "text/event-stream")];
        for _ in 0..20 {
            script.push(Step::chunk("tick"));
            script.push(Step::pause(Duration::from_millis(100)));
        }
        script.push(Step::end());
        script
    }

    #[tokio::test]
    async fn streaming_client_bounds_silence_not_total_time() {
        let server =
            ScriptedServer::start(vec![slow_steady_response(), slow_steady_response()]).await;
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(1)).unwrap();

        let streamed = http
            .streaming_client()
            .get(server.uri())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(
            streamed,
            "tick".repeat(20),
            "a body that keeps arriving outlives the timeout"
        );

        let whole = async { http.client().get(server.uri()).send().await?.text().await }.await;
        assert!(
            whole.is_err_and(|e| e.is_timeout()),
            "the same exchange through the whole-request client times out"
        );
    }

    #[tokio::test]
    async fn broken_stream_body_reads_as_an_interrupted_stream() {
        let mut script = vec![Step::head(200, "text/event-stream")];
        script.push(Step::chunk("partial"));
        let server = ScriptedServer::start(vec![script]).await;
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();

        let body = http
            .streaming_client()
            .get(server.uri())
            .send()
            .await
            .unwrap()
            .text()
            .await;
        let error = map_stream_read_error(body.unwrap_err(), 5);
        assert!(
            matches!(&error, InferenceError::StreamInterrupted(detail) if !detail.is_empty()),
            "a body cut off mid-chunk is an interrupted stream with a reason: {error:?}"
        );
    }

    /// A send that fails at the transport, carrying a credential in its
    /// query. Binds a port and frees it, so the connection is refused at once.
    async fn refused_send(query: &str) -> reqwest::Error {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let error = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/models?{query}"))
            .send()
            .await
            .unwrap_err();
        assert!(error.url().is_some(), "the fixture error names its URL");
        error
    }

    #[tokio::test]
    async fn mapped_errors_never_name_the_url_or_its_query() {
        const SECRET: &str = "key=hunter2-secret";
        let mapped = [
            map_request_error(refused_send(SECRET).await, 5),
            map_stream_request_error(refused_send(SECRET).await, 5),
            map_stream_read_error(refused_send(SECRET).await, 5),
        ];
        for error in mapped {
            let text = error.to_string();
            assert!(
                !text.contains("hunter2") && !text.contains("://") && !text.contains("127.0.0.1"),
                "a mapped error names no URL or query: {text}"
            );
            assert!(
                text.contains("error sending request") || text.contains("connect"),
                "the cause is kept: {text}"
            );
        }
    }

    struct FieldFormatter(String);
    impl tracing::field::Visit for FieldFormatter {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write as _;
            write!(self.0, "{}={value:?} ", field.name()).unwrap();
        }
    }

    struct EventCollector {
        max_level: tracing::Level,
        events: std::sync::Mutex<Vec<(tracing::Level, String)>>,
    }

    impl tracing::Subscriber for EventCollector {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.level() <= &self.max_level
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            if !self.enabled(event.metadata()) {
                return;
            }
            let mut visitor = FieldFormatter(String::new());
            event.record(&mut visitor);
            self.events
                .lock()
                .unwrap()
                .push((*event.metadata().level(), visitor.0));
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[test]
    fn retryable_errors_omit_request_body_above_trace_while_client_errors_log_at_debug() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(498));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(422));

        let collector = Arc::new(EventCollector {
            max_level: tracing::Level::DEBUG,
            events: std::sync::Mutex::new(Vec::new()),
        });
        let req_payload = r#"{"model":"glm-5.3","messages":[{"role":"system","content":"secret-system-prompt"}]}"#;
        tracing::subscriber::with_default(Arc::clone(&collector), || {
            log_api_error(
                "openai",
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":{"message":"rate limit exceeded"}}"#,
                req_payload,
            );
            log_api_error(
                "openai",
                reqwest::StatusCode::BAD_REQUEST,
                r#"{"error":{"message":"invalid schema"}}"#,
                req_payload,
            );
        });

        let events = collector.events.lock().unwrap().clone();
        // At DEBUG max level, 429 produces only the WARN event (without request_body),
        // while 400 produces WARN (without request_body) + DEBUG (with request_body).
        let warn_events: Vec<&String> = events
            .iter()
            .filter(|(lvl, _)| *lvl == tracing::Level::WARN)
            .map(|(_, text)| text)
            .collect();
        assert_eq!(warn_events.len(), 2);
        for w in &warn_events {
            assert!(
                !w.contains("secret-system-prompt"),
                "WARN log must never contain request_body: {w}"
            );
            assert!(w.contains("request_bytes="), "WARN log records size: {w}");
        }

        let debug_events: Vec<&String> = events
            .iter()
            .filter(|(lvl, _)| *lvl == tracing::Level::DEBUG)
            .map(|(_, text)| text)
            .collect();
        assert_eq!(
            debug_events.len(),
            1,
            "only non-retryable 400 logs request_body at DEBUG"
        );
        assert!(
            debug_events
                .first()
                .unwrap()
                .contains("secret-system-prompt")
        );
    }
}
