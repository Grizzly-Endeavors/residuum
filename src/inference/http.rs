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
    pub pool_max_idle_per_host: usize,
    /// HTTP/2 keep-alive interval in seconds (default: 30).
    pub http2_keep_alive_secs: u64,
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
    pub fn streaming_client(&self) -> &Client {
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
pub fn map_request_error(e: reqwest::Error, timeout_secs: u64) -> InferenceError {
    if e.is_timeout() {
        InferenceError::Timeout(timeout_secs)
    } else {
        InferenceError::Request(e)
    }
}

/// Map an error from sending a streaming request to a [`InferenceError`].
pub fn map_stream_request_error(e: reqwest::Error, idle_secs: u64) -> InferenceError {
    if e.is_timeout() {
        InferenceError::Stalled(idle_secs)
    } else {
        InferenceError::Request(e)
    }
}

/// Map an error from reading a streaming response body: the model going
/// quiet is a stall, anything else (a dropped connection, a body that ends
/// mid-chunk) an interrupted stream.
pub fn map_stream_read_error(e: &reqwest::Error, idle_secs: u64) -> InferenceError {
    if e.is_timeout() {
        InferenceError::Stalled(idle_secs)
    } else {
        InferenceError::StreamInterrupted(error_chain(e))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
