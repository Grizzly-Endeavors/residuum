//! HTTP client for the System 1 `/v1/systemone` API.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;
use tracing::{debug, warn};

use super::error::SystemOneError;
use super::types::{
    ModelListResponse, Question, SystemOneModel, SystemOneRequest, SystemOneResponse,
};
use crate::inference::http::{SharedHttpClient, read_error_body, warn_if_insecure_remote};
use crate::inference::retry::RetryConfig;

/// Where a System 1 model is served and how to call it.
#[derive(Clone, PartialEq, Eq)]
pub struct SystemOneEndpoint {
    /// Display name used in messages: `TypeSafe`, Ollama, or a host.
    pub provider: String,
    /// Base URL without the `/v1/...` path.
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    /// Ollama's `keep_alive`, sent only when set.
    pub keep_alive: Option<String>,
}

impl std::fmt::Debug for SystemOneEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemOneEndpoint")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("keep_alive", &self.keep_alive)
            .finish()
    }
}

/// A client for one configured System 1 endpoint. Cheap to clone.
#[derive(Clone)]
pub struct SystemOneClient {
    http: SharedHttpClient,
    endpoint: Arc<SystemOneEndpoint>,
    retry: RetryConfig,
}

impl SystemOneClient {
    /// Create a client for `endpoint` with the default retry policy.
    #[must_use]
    pub fn new(http: SharedHttpClient, endpoint: SystemOneEndpoint) -> Self {
        warn_if_insecure_remote(&endpoint.base_url);
        Self {
            http,
            endpoint: Arc::new(endpoint),
            retry: RetryConfig::default(),
        }
    }

    /// Replace the retry policy.
    #[must_use]
    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    #[must_use]
    pub fn endpoint(&self) -> &SystemOneEndpoint {
        &self.endpoint
    }

    /// Ask `questions` about `state` in one request.
    ///
    /// # Errors
    /// Returns a [`SystemOneError`] when the service can't be reached, refuses
    /// the request, or answers with something unreadable. Rate limits,
    /// overloads, and network failures are retried with backoff first.
    #[tracing::instrument(skip_all, fields(
        system_one.provider = %self.endpoint.provider,
        system_one.model = %self.endpoint.model,
        questions = questions.len(),
    ))]
    pub async fn evaluate(
        &self,
        state: &Value,
        questions: &BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, SystemOneError> {
        let body = SystemOneRequest {
            model: &self.endpoint.model,
            state,
            questions,
            keep_alive: self.endpoint.keep_alive.as_deref(),
        };
        let url = self.url("systemone");
        let response = self
            .retrying(|| async {
                let request = self.authorize(self.http.client().post(&url)).json(&body);
                let text = self.send(request, &url).await?;
                serde_json::from_str::<SystemOneResponse>(&text).map_err(|e| {
                    SystemOneError::InvalidResponse {
                        provider: self.endpoint.provider.clone(),
                        detail: e.to_string(),
                    }
                })
            })
            .await?;
        debug!(
            answered_by = %response.model,
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            "system one evaluation complete"
        );
        Ok(response)
    }

    /// List the model names this endpoint accepts.
    ///
    /// # Errors
    /// Returns a [`SystemOneError`] when the list can't be fetched or read.
    pub async fn list_models(&self) -> Result<Vec<SystemOneModel>, SystemOneError> {
        let url = self.url("models");
        let request = self.authorize(self.http.client().get(&url));
        let text = self.send(request, &url).await?;
        let list: ModelListResponse =
            serde_json::from_str(&text).map_err(|e| SystemOneError::InvalidResponse {
                provider: self.endpoint.provider.clone(),
                detail: e.to_string(),
            })?;
        Ok(list.into_models())
    }

    fn url(&self, path: &str) -> String {
        let base = self.endpoint.base_url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        format!("{base}/v1/{path}")
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.endpoint.api_key {
            Some(key) if !key.is_empty() => request.bearer_auth(key),
            _ => request,
        }
    }

    /// Send a request and return the body of a successful response.
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        url: &str,
    ) -> Result<String, SystemOneError> {
        let provider = &self.endpoint.provider;
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                SystemOneError::Timeout {
                    provider: provider.clone(),
                    secs: self.http.timeout_secs(),
                }
            } else {
                SystemOneError::Unreachable {
                    provider: provider.clone(),
                    url: self.endpoint.base_url.clone(),
                    detail: e.to_string(),
                }
            }
        })?;
        let status = response.status();
        if status.is_success() {
            return response
                .text()
                .await
                .map_err(|e| SystemOneError::Unreachable {
                    provider: provider.clone(),
                    url: self.endpoint.base_url.clone(),
                    detail: format!("the response was cut off: {e}"),
                });
        }
        let body = read_error_body(response).await;
        warn!(
            status = %status,
            url = %url,
            response_body = %body,
            provider = %provider,
            "system one API error"
        );
        Err(self.classify_status(status, body))
    }

    fn classify_status(&self, status: StatusCode, body: String) -> SystemOneError {
        let provider = self.endpoint.provider.clone();
        let lower = body.to_lowercase();
        match status.as_u16() {
            401 | 403 => SystemOneError::AuthRejected { provider },
            429 => SystemOneError::RateLimited { provider },
            404 => SystemOneError::ModelNotFound {
                provider,
                model: self.endpoint.model.clone(),
                detail: body,
            },
            // Ollama answers 400 with "model not found" for an unpulled model.
            _ if lower.contains("not found") && lower.contains("model") => {
                SystemOneError::ModelNotFound {
                    provider,
                    model: self.endpoint.model.clone(),
                    detail: body,
                }
            }
            // 529 is TypeSafe's "overloaded".
            500..=599 => SystemOneError::Overloaded {
                provider,
                detail: body,
            },
            _ => SystemOneError::RequestRejected {
                provider,
                detail: format!("{status}: {body}"),
            },
        }
    }

    /// Run `operation`, retrying transient failures with exponential backoff.
    /// Logs once when retries start and once if they run out.
    async fn retrying<F, Fut, T>(&self, operation: F) -> Result<T, SystemOneError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, SystemOneError>>,
    {
        let mut attempts = 0_u32;
        let mut delay = self.retry.initial_delay;
        loop {
            match operation().await {
                Ok(value) => {
                    if attempts > 0 {
                        debug!(
                            attempts = attempts + 1,
                            "system one call recovered after retrying"
                        );
                    }
                    return Ok(value);
                }
                Err(e) if !e.is_retryable() => return Err(e),
                Err(e) if attempts >= self.retry.max_retries => {
                    warn!(attempts = attempts + 1, error = %e, "system one retries exhausted");
                    return Err(e);
                }
                Err(e) => {
                    if attempts == 0 {
                        warn!(
                            max_retries = self.retry.max_retries,
                            error = %e,
                            "system one call failed, retrying"
                        );
                    }
                    attempts += 1;
                    tokio::time::sleep(delay).await;
                    delay = next_delay(delay, &self.retry);
                }
            }
        }
    }
}

fn next_delay(delay: Duration, retry: &RetryConfig) -> Duration {
    delay.mul_f64(retry.backoff_multiplier).min(retry.max_delay)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::inference::HttpClientConfig;
    use crate::inference::system_one::Answer;

    fn client(server: &MockServer, api_key: Option<&str>) -> SystemOneClient {
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();
        SystemOneClient::new(
            http,
            SystemOneEndpoint {
                provider: "TypeSafe".to_string(),
                base_url: server.uri(),
                model: "jev-latest".to_string(),
                api_key: api_key.map(str::to_string),
                keep_alive: None,
            },
        )
        .with_retry(RetryConfig {
            max_retries: 2,
            initial_delay: Duration::from_millis(5),
            max_delay: Duration::from_millis(20),
            backoff_multiplier: 2.0,
        })
    }

    fn one_question() -> BTreeMap<String, Question> {
        BTreeMap::from([("urgent".to_string(), Question::noul("Is this urgent?"))])
    }

    fn noul_body(model: &str) -> serde_json::Value {
        json!({
            "model": model,
            "answers": { "urgent": { "type": "noul", "noul": 0.95 } },
            "usage": { "input_tokens": 12, "output_tokens": 1 }
        })
    }

    #[tokio::test]
    async fn evaluate_sends_bearer_key_and_parses_answers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(header("authorization", "Bearer sk-test"))
            .and(body_partial_json(json!({
                "model": "jev-latest",
                "state": "Payouts failing",
                "questions": { "urgent": { "type": "noul" } }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(noul_body("jev-1.13.0")))
            .expect(1)
            .mount(&server)
            .await;

        let response = client(&server, Some("sk-test"))
            .evaluate(&json!("Payouts failing"), &one_question())
            .await
            .unwrap();
        assert_eq!(response.model, "jev-1.13.0", "answering model is reported");
        assert_eq!(
            response.answers.get("urgent").and_then(Answer::noul),
            Some(0.95),
            "noul answer parses"
        );
    }

    #[tokio::test]
    async fn evaluate_without_key_sends_no_auth_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(|req: &Request| {
                if req.headers.contains_key("authorization") {
                    ResponseTemplate::new(400)
                } else {
                    ResponseTemplate::new(200).set_body_json(noul_body("nimble"))
                }
            })
            .mount(&server)
            .await;

        let result = client(&server, None)
            .evaluate(&json!("x"), &one_question())
            .await;
        assert!(
            result.is_ok(),
            "ollama-style call without a key: {result:?}"
        );
    }

    #[tokio::test]
    async fn base_url_ending_in_v1_is_not_doubled() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(noul_body("nimble")))
            .expect(1)
            .mount(&server)
            .await;
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();
        let c = SystemOneClient::new(
            http,
            SystemOneEndpoint {
                provider: "Ollama".to_string(),
                base_url: format!("{}/v1/", server.uri()),
                model: "nimble".to_string(),
                api_key: None,
                keep_alive: Some("5m".to_string()),
            },
        );
        c.evaluate(&json!("x"), &one_question()).await.unwrap();
    }

    #[tokio::test]
    async fn rate_limit_is_retried_until_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(noul_body("jev-1.13.0")))
            .mount(&server)
            .await;

        let result = client(&server, Some("k"))
            .evaluate(&json!("x"), &one_question())
            .await;
        assert!(result.is_ok(), "429 then 200 should succeed: {result:?}");
    }

    #[tokio::test]
    async fn bad_key_is_not_retried_and_reads_plainly() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid key"))
            .expect(1)
            .mount(&server)
            .await;

        let err = client(&server, Some("wrong"))
            .evaluate(&json!("x"), &one_question())
            .await
            .unwrap_err();
        assert!(
            matches!(err, SystemOneError::AuthRejected { .. }),
            "401 should map to AuthRejected, got {err:?}"
        );
        assert!(
            err.user_message().contains("API key"),
            "message should name the key: {}",
            err.user_message()
        );
        assert!(err.affects_health(), "a bad key is a service-level outage");
    }

    #[tokio::test]
    async fn unprocessable_request_does_not_affect_health() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(422).set_body_string("state too long"))
            .mount(&server)
            .await;
        let err = client(&server, Some("k"))
            .evaluate(&json!("x"), &one_question())
            .await
            .unwrap_err();
        assert!(
            matches!(err, SystemOneError::RequestRejected { .. }),
            "422 is a per-request rejection, got {err:?}"
        );
        assert!(!err.affects_health(), "one bad request isn't an outage");
    }

    #[tokio::test]
    async fn unreachable_server_is_reported_as_unreachable() {
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(2)).unwrap();
        let c = SystemOneClient::new(
            http,
            SystemOneEndpoint {
                provider: "Ollama".to_string(),
                base_url: "http://unreachable.invalid".to_string(),
                model: "nimble".to_string(),
                api_key: None,
                keep_alive: None,
            },
        )
        .with_retry(RetryConfig::no_retry());
        let err = c.evaluate(&json!("x"), &one_question()).await.unwrap_err();
        assert!(
            matches!(err, SystemOneError::Unreachable { .. }),
            "unreachable server should be Unreachable, got {err:?}"
        );
    }

    #[tokio::test]
    async fn list_models_reads_typesafe_and_ollama_shapes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models": [{ "name": "jev-latest", "description": "flagship" }],
                "data": [{ "id": "nimble" }]
            })))
            .mount(&server)
            .await;
        let names: Vec<String> = client(&server, Some("k"))
            .list_models()
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names, vec!["jev-latest", "nimble"], "both shapes are read");
    }
}
