//! Webhook interface adapter — accepts HTTP POST requests and routes them to the agent.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use tokio::sync::mpsc;

use super::types::{InboundMessage, MessageOrigin, ReplyHandle, RoutedMessage};

/// Shared state for the webhook handler.
#[derive(Clone)]
pub struct WebhookState {
    /// Sender for routing messages to the agent main loop.
    pub inbound_tx: mpsc::Sender<RoutedMessage>,
    /// Optional bearer token for authentication.
    pub secret: Option<String>,
}

/// JSON payload for webhook requests.
#[derive(serde::Deserialize)]
struct WebhookPayload {
    content: String,
}

/// Axum handler for `POST /webhook`.
///
/// Accepts either `application/json` with `{ "content": "..." }` or plain text body.
/// If a secret is configured, validates the `Authorization: Bearer <secret>` header.
/// Returns 202 Accepted immediately — responses are fire-and-forget.
pub async fn webhook_handler(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Validate bearer token if secret is configured
    if let Some(ref expected) = state.secret {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let provided = auth.strip_prefix("Bearer ").unwrap_or("");
        if provided != expected.as_str() {
            return StatusCode::UNAUTHORIZED;
        }
    }

    // Parse body as JSON or plain text
    let content = match serde_json::from_slice::<WebhookPayload>(&body) {
        Ok(payload) => payload.content,
        Err(e) => {
            tracing::debug!(error = %e, "webhook body is not valid JSON, falling back to plain text");
            // Try as plain text
            match String::from_utf8(body.to_vec()) {
                Ok(text) if !text.trim().is_empty() => text,
                _ => return StatusCode::BAD_REQUEST,
            }
        }
    };

    let origin = MessageOrigin {
        endpoint: "webhook".to_string(),
        sender_name: "webhook".to_string(),
        sender_id: "webhook".to_string(),
    };

    let inbound = InboundMessage {
        id: uuid::Uuid::new_v4().to_string(),
        content,
        origin,
        timestamp: chrono::Utc::now(),
        images: vec![],
    };

    let reply = Arc::new(WebhookReplyHandle);

    let routed = RoutedMessage {
        message: inbound,
        reply,
    };

    if state.inbound_tx.send(routed).await.is_err() {
        tracing::warn!("inbound channel closed, dropping webhook message");
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    StatusCode::ACCEPTED
}

/// Fire-and-forget reply handle for webhook requests.
///
/// Webhook responses are logged but have no return path — the HTTP
/// response (202 Accepted) is sent before the agent processes the message.
struct WebhookReplyHandle;

#[async_trait::async_trait]
impl ReplyHandle for WebhookReplyHandle {
    async fn send_response(&self, content: &str) {
        tracing::info!(
            channel = "webhook",
            response_len = content.len(),
            "webhook response (no return channel)"
        );
    }

    async fn send_typing(&self) {
        // No typing indicator for webhooks
    }

    async fn send_system_event(&self, source: &str, content: &str) {
        tracing::info!(
            channel = "webhook",
            source,
            event_len = content.len(),
            "webhook system event (no return channel)"
        );
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    fn make_app(secret: Option<String>) -> (axum::Router, mpsc::Receiver<RoutedMessage>) {
        let (tx, rx) = mpsc::channel::<RoutedMessage>(32);
        let state = WebhookState {
            inbound_tx: tx,
            secret,
        };
        let app = axum::Router::new()
            .route("/webhook", post(webhook_handler))
            .with_state(state);
        (app, rx)
    }

    #[tokio::test]
    async fn json_payload_accepted() {
        let (app, mut rx) = make_app(None);

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"content":"hello from webhook"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED, "should return 202");

        let routed = rx.try_recv().unwrap();
        assert_eq!(
            routed.message.content, "hello from webhook",
            "content should match"
        );
        assert_eq!(
            routed.message.origin.endpoint, "webhook",
            "endpoint should be webhook"
        );
    }

    #[tokio::test]
    async fn plain_text_accepted() {
        let (app, mut rx) = make_app(None);

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(Body::from("plain text message"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "should accept plain text"
        );

        let routed = rx.try_recv().unwrap();
        assert_eq!(
            routed.message.content, "plain text message",
            "content should match"
        );
    }

    #[tokio::test]
    async fn empty_body_rejected() {
        let (app, _rx) = make_app(None);

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .body(Body::from(""))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "empty body should be rejected"
        );
    }

    #[tokio::test]
    async fn bearer_auth_required() {
        let (app, _rx) = make_app(Some("secret-token".to_string()));

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "missing auth should be rejected"
        );
    }

    #[tokio::test]
    async fn bearer_auth_wrong_token() {
        let (app, _rx) = make_app(Some("secret-token".to_string()));

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("authorization", "Bearer wrong-token")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "wrong token should be rejected"
        );
    }

    #[tokio::test]
    async fn bearer_auth_correct_token() {
        let (app, mut rx) = make_app(Some("secret-token".to_string()));

        let req = Request::builder()
            .method("POST")
            .uri("/webhook")
            .header("content-type", "application/json")
            .header("authorization", "Bearer secret-token")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "correct token should be accepted"
        );

        let routed = rx.try_recv().unwrap();
        assert_eq!(routed.message.content, "hello", "content should match");
    }
}
