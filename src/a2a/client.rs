//! A2A protocol client for communicating with remote A2A agents.
//!
//! Provides discovery (fetching Agent Cards) and task delegation
//! (sending tasks via JSON-RPC 2.0 `tasks/send`).

use std::collections::HashMap;
use std::time::Duration;

use reqwest::Client;

use super::types::{A2aRole, AgentCard, JsonRpcId, Part, Task};

/// A2A client error.
#[derive(Debug, thiserror::Error)]
pub enum A2aClientError {
    /// An HTTP request to the remote agent failed.
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The remote agent returned an unparseable or unexpected response.
    #[error("invalid response from remote agent: {0}")]
    InvalidResponse(String),
    /// The remote agent returned a JSON-RPC error.
    #[error("remote agent returned JSON-RPC error: code={code}, message={message}")]
    JsonRpcError {
        /// Numeric error code from the JSON-RPC error object.
        code: i32,
        /// Human-readable error message from the JSON-RPC error object.
        message: String,
    },
}

/// Client for interacting with remote A2A agents.
///
/// Supports agent discovery via Agent Cards and task delegation
/// via JSON-RPC 2.0 `tasks/send`.
pub struct A2aClient {
    client: Client,
}

impl A2aClient {
    /// Create a new A2A client with a 60-second request timeout.
    ///
    /// # Errors
    ///
    /// Returns `A2aClientError::Http` if the underlying HTTP client cannot be built.
    pub fn new() -> Result<Self, A2aClientError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self { client })
    }

    /// Discover a remote agent by fetching its Agent Card.
    ///
    /// Sends a GET request to `{base_url}/.well-known/agent.json` and
    /// deserializes the response as an [`AgentCard`].
    ///
    /// # Errors
    ///
    /// Returns `A2aClientError::Http` if the request fails, or
    /// `A2aClientError::InvalidResponse` if the response cannot be parsed.
    pub async fn discover(&self, base_url: &str) -> Result<AgentCard, A2aClientError> {
        let url = format!("{base_url}/.well-known/agent.json");
        tracing::debug!(url = %url, "fetching agent card");

        let response = self.client.get(&url).send().await?;
        let status = response.status();

        if !status.is_success() {
            return Err(A2aClientError::InvalidResponse(format!(
                "agent card request returned HTTP {status}"
            )));
        }

        let card: AgentCard = response.json().await.map_err(|e| {
            A2aClientError::InvalidResponse(format!("failed to parse agent card: {e}"))
        })?;

        tracing::debug!(agent_name = %card.name, "discovered remote agent");
        Ok(card)
    }

    /// Send a task to a remote A2A agent via JSON-RPC 2.0 `tasks/send`.
    ///
    /// Builds a `TaskSendParams` with a user-role message containing the given
    /// text, wraps it in a JSON-RPC 2.0 request envelope, and POSTs it to
    /// `{base_url}/a2a`.
    ///
    /// # Errors
    ///
    /// Returns `A2aClientError::Http` if the request fails,
    /// `A2aClientError::JsonRpcError` if the remote agent returns a JSON-RPC error, or
    /// `A2aClientError::InvalidResponse` if the response cannot be parsed.
    pub async fn send_task(
        &self,
        base_url: &str,
        message_text: &str,
        task_id: &str,
        session_id: Option<&str>,
        secret: Option<&str>,
    ) -> Result<Task, A2aClientError> {
        let url = format!("{base_url}/a2a");

        let message = serde_json::json!({
            "role": A2aRole::User,
            "parts": [Part::Text { text: message_text.to_string() }],
            "metadata": HashMap::<String, serde_json::Value>::new(),
        });

        let params = serde_json::json!({
            "id": task_id,
            "sessionId": session_id,
            "message": message,
            "metadata": {},
        });

        let request_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": JsonRpcId::String(task_id.to_string()),
            "method": "tasks/send",
            "params": params,
        });

        tracing::debug!(
            url = %url,
            task_id = %task_id,
            "sending task to remote agent"
        );

        let mut req = self.client.post(&url).json(&request_body);

        if let Some(token) = secret {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        let response = req.send().await?;
        let status = response.status();

        if !status.is_success() {
            return Err(A2aClientError::InvalidResponse(format!(
                "tasks/send request returned HTTP {status}"
            )));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            A2aClientError::InvalidResponse(format!("failed to parse response body: {e}"))
        })?;

        // Check for JSON-RPC error
        if let Some(error) = body.get("error") {
            let code = error
                .get("code")
                .and_then(serde_json::Value::as_i64)
                .map_or(0, |c| {
                    i32::try_from(c).unwrap_or(0)
                });
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error")
                .to_string();

            return Err(A2aClientError::JsonRpcError { code, message });
        }

        // Extract result field
        let result = body.get("result").ok_or_else(|| {
            A2aClientError::InvalidResponse(
                "response contains neither 'result' nor 'error' field".to_string(),
            )
        })?;

        let task: Task = serde_json::from_value(result.clone()).map_err(|e| {
            A2aClientError::InvalidResponse(format!("failed to parse task from result: {e}"))
        })?;

        tracing::debug!(
            task_id = %task.id,
            state = ?task.status.state,
            "received task response from remote agent"
        );

        Ok(task)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::panic, reason = "test code panics on unexpected match arm")]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn new_creates_client() {
        let client = A2aClient::new();
        assert!(client.is_ok(), "client should be created successfully");
    }

    #[tokio::test]
    async fn discover_fetches_agent_card() {
        let server = MockServer::start().await;

        let card_json = serde_json::json!({
            "name": "Remote Agent",
            "description": "A test remote agent",
            "url": "http://localhost/a2a",
            "version": "0.2",
            "capabilities": {
                "streaming": false,
                "pushNotifications": false,
                "stateTransitionHistory": false
            },
            "skills": [],
            "defaultInputModes": ["text/plain"],
            "defaultOutputModes": ["text/plain"]
        });

        Mock::given(method("GET"))
            .and(path("/.well-known/agent.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&card_json))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let card = client.discover(&server.uri()).await.unwrap();

        assert_eq!(card.name, "Remote Agent", "agent name should match");
        assert_eq!(card.version, "0.2", "version should match");
    }

    #[tokio::test]
    async fn discover_returns_error_on_404() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/.well-known/agent.json"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let result = client.discover(&server.uri()).await;

        assert!(result.is_err(), "discover should fail on 404");
        let err = result.unwrap_err();
        assert!(
            matches!(err, A2aClientError::InvalidResponse(_)),
            "should be InvalidResponse error, got: {err}"
        );
    }

    #[tokio::test]
    async fn discover_returns_error_on_invalid_json() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/.well-known/agent.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not valid json"))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let result = client.discover(&server.uri()).await;

        assert!(result.is_err(), "discover should fail on invalid JSON");
        let err = result.unwrap_err();
        assert!(
            matches!(err, A2aClientError::InvalidResponse(_)),
            "should be InvalidResponse error, got: {err}"
        );
    }

    #[tokio::test]
    async fn send_task_builds_correct_jsonrpc_request() {
        let server = MockServer::start().await;

        let task_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "task-1",
            "result": {
                "id": "task-1",
                "status": {
                    "state": "completed",
                    "message": {
                        "role": "agent",
                        "parts": [{"type": "text", "text": "done"}]
                    }
                },
                "history": [],
                "artifacts": []
            }
        });

        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&task_response))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let task = client
            .send_task(&server.uri(), "hello agent", "task-1", None, None)
            .await
            .unwrap();

        assert_eq!(task.id, "task-1", "task id should match");
        assert_eq!(
            task.status.state,
            super::super::types::TaskState::Completed,
            "task state should be completed"
        );
    }

    #[tokio::test]
    async fn send_task_includes_session_id() {
        let server = MockServer::start().await;

        let task_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "task-2",
            "result": {
                "id": "task-2",
                "sessionId": "session-abc",
                "status": { "state": "completed" },
                "history": [],
                "artifacts": []
            }
        });

        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&task_response))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let task = client
            .send_task(
                &server.uri(),
                "hello",
                "task-2",
                Some("session-abc"),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            task.session_id.as_deref(),
            Some("session-abc"),
            "session id should match"
        );
    }

    #[tokio::test]
    async fn send_task_includes_auth_header() {
        let server = MockServer::start().await;

        let task_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "task-3",
            "result": {
                "id": "task-3",
                "status": { "state": "completed" },
                "history": [],
                "artifacts": []
            }
        });

        Mock::given(method("POST"))
            .and(path("/a2a"))
            .and(header("Authorization", "Bearer my-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&task_response))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let task = client
            .send_task(
                &server.uri(),
                "hello",
                "task-3",
                None,
                Some("my-secret"),
            )
            .await
            .unwrap();

        assert_eq!(task.id, "task-3", "task should succeed with correct auth");
    }

    #[tokio::test]
    async fn send_task_returns_jsonrpc_error() {
        let server = MockServer::start().await;

        let error_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "task-err",
            "error": {
                "code": -32601,
                "message": "method not found"
            }
        });

        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&error_response))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let result = client
            .send_task(&server.uri(), "hello", "task-err", None, None)
            .await;

        assert!(result.is_err(), "should return error on JSON-RPC error");
        let err = result.unwrap_err();
        match err {
            A2aClientError::JsonRpcError { code, message } => {
                assert_eq!(code, -32601, "error code should match");
                assert_eq!(message, "method not found", "error message should match");
            }
            other => panic!("expected JsonRpcError, got: {other}"),
        }
    }

    #[tokio::test]
    async fn send_task_returns_error_on_http_failure() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let result = client
            .send_task(&server.uri(), "hello", "task-fail", None, None)
            .await;

        assert!(result.is_err(), "should return error on HTTP 500");
        let err = result.unwrap_err();
        assert!(
            matches!(err, A2aClientError::InvalidResponse(_)),
            "should be InvalidResponse error, got: {err}"
        );
    }

    #[tokio::test]
    async fn send_task_returns_error_on_missing_result() {
        let server = MockServer::start().await;

        let bad_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "task-bad"
        });

        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&bad_response))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let result = client
            .send_task(&server.uri(), "hello", "task-bad", None, None)
            .await;

        assert!(
            result.is_err(),
            "should return error when result field is missing"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, A2aClientError::InvalidResponse(_)),
            "should be InvalidResponse error, got: {err}"
        );
    }

    #[tokio::test]
    async fn send_task_returns_error_on_invalid_result() {
        let server = MockServer::start().await;

        let bad_response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "task-bad",
            "result": "not a task object"
        });

        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&bad_response))
            .mount(&server)
            .await;

        let client = A2aClient::new().unwrap();
        let result = client
            .send_task(&server.uri(), "hello", "task-bad", None, None)
            .await;

        assert!(
            result.is_err(),
            "should return error when result is not a valid task"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, A2aClientError::InvalidResponse(_)),
            "should be InvalidResponse error, got: {err}"
        );
    }
}
