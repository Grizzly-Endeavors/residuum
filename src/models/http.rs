//! Shared HTTP utilities for model provider clients.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;

use super::ModelError;

/// Configuration for HTTP client connection pooling.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Request timeout in seconds.
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
    timeout_secs: u64,
}

impl SharedHttpClient {
    /// Create a new shared HTTP client with the specified configuration.
    ///
    /// # Errors
    /// Returns `ModelError::Request` if the HTTP client cannot be built.
    pub fn new(config: &HttpClientConfig) -> Result<Self, ModelError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .http2_keep_alive_interval(Duration::from_secs(config.http2_keep_alive_secs))
            .build()?;

        Ok(Self {
            client: Arc::new(client),
            timeout_secs: config.timeout_secs,
        })
    }

    /// Get a reference to the underlying HTTP client.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
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

/// Map a reqwest error to a [`ModelError`], detecting timeouts.
pub fn map_request_error(e: reqwest::Error, timeout_secs: u64) -> ModelError {
    if e.is_timeout() {
        ModelError::Timeout(timeout_secs)
    } else {
        ModelError::Request(e)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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
