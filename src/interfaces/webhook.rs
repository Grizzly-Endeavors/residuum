//! Webhook interface adapter — accepts HTTP POST requests and routes them to the agent.
//!
//! Each named webhook (`/webhook/{name}`) has its own secret, format, and content
//! field extraction config. Unknown names return 404.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;

use crate::config::{WebhookFormat, WebhookRouting};

/// Per-webhook endpoint configuration (built from `WebhookEntry` at startup).
#[derive(Clone, Debug)]
pub struct WebhookEndpointState {
    /// Optional bearer token for authentication.
    pub secret: Option<String>,
    /// Payload extraction format.
    pub format: WebhookFormat,
    /// JSON dot-path fields to extract (only used with `Parsed` format).
    pub content_fields: Option<Vec<String>>,
    /// Routing destination for this webhook.
    pub routing: WebhookRouting,
}

/// Shared state for the webhook handler — holds all named webhook configs.
#[derive(Clone)]
pub struct WebhookState {
    /// Publisher for sending events onto the bus.
    pub publisher: crate::bus::Publisher,
    /// Named webhook endpoint configurations.
    pub webhooks: HashMap<String, WebhookEndpointState>,
}

/// Axum handler for `POST /webhook/{name}`.
///
/// Looks up the named webhook config, validates auth, extracts content per the
/// webhook's format / content-fields settings, and publishes a `NotificationEvent`.
/// Returns 404 for unknown names, 401 for bad auth, 202 on success.
pub async fn webhook_handler(
    State(state): State<WebhookState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(endpoint) = state.webhooks.get(&name) else {
        return StatusCode::NOT_FOUND;
    };

    tracing::debug!(webhook = %name, content_length = body.len(), "webhook request received");

    // Validate bearer token if secret is configured
    if let Some(ref expected) = endpoint.secret {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let provided = auth.strip_prefix("Bearer ").unwrap_or("");
        if provided != expected.as_str() {
            tracing::warn!(webhook = %name, "webhook authentication failed");
            return StatusCode::UNAUTHORIZED;
        }
    }

    // Extract content based on format
    let content = match endpoint.format {
        WebhookFormat::Raw => match String::from_utf8(body.to_vec()) {
            Ok(text) if !text.trim().is_empty() => text,
            _ => return StatusCode::BAD_REQUEST,
        },
        WebhookFormat::Parsed => {
            match extract_parsed_content(&body, endpoint.content_fields.as_deref()) {
                Some(text) => text,
                None => return StatusCode::BAD_REQUEST,
            }
        }
    };

    let notification = crate::bus::NotificationEvent {
        title: format!("webhook:{name}"),
        content,
        source: crate::bus::EventTrigger::Webhook(name.clone()),
        timestamp: crate::time::now_local(chrono_tz::UTC),
    };

    match &endpoint.routing {
        WebhookRouting::Inbox => {
            if let Err(e) = state
                .publisher
                .publish(crate::bus::topics::Inbox, notification)
                .await
            {
                tracing::warn!(webhook = %name, error = %e, "bus publish failed, dropping webhook message");
                return StatusCode::SERVICE_UNAVAILABLE;
            }
        }
        WebhookRouting::Agent(preset) => {
            let spawn_event = crate::bus::SpawnRequestEvent {
                source_label: format!("webhook:{name}"),
                prompt: notification.content,
                context: None,
                source: crate::bus::EventTrigger::Webhook(name.clone()),
                model_tier_override: None,
            };
            let topic =
                crate::bus::topics::SpawnRequest(crate::bus::PresetName::from(preset.as_str()));
            if let Err(e) = state.publisher.publish(topic, spawn_event).await {
                tracing::warn!(webhook = %name, error = %e, "bus publish failed, dropping webhook message");
                return StatusCode::SERVICE_UNAVAILABLE;
            }
        }
    }

    tracing::debug!(webhook = %name, "webhook message published");
    StatusCode::ACCEPTED
}

/// Extract content from the body using parsed format rules.
///
/// - With `content_fields`: parse JSON, extract each dot-path, join with `\n\n`
/// - Without `content_fields`: extract `"content"` field from JSON, fallback to plain text
fn extract_parsed_content(body: &[u8], content_fields: Option<&[String]>) -> Option<String> {
    if let Some(fields) = content_fields {
        // Must be JSON when content_fields are specified
        let json: serde_json::Value = serde_json::from_slice(body).ok()?;
        let parts: Vec<String> = fields
            .iter()
            .filter_map(|path| extract_json_field(&json, path))
            .collect();
        if parts.is_empty() {
            return None;
        }
        Some(parts.join("\n\n"))
    } else {
        // Default: try JSON { "content": "..." }, fallback to plain text
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(body)
            && let Some(content) = json.get("content").and_then(|v| v.as_str())
            && !content.trim().is_empty()
        {
            return Some(content.to_string());
        }
        // Fallback to plain text
        let text = String::from_utf8(body.to_vec()).ok()?;
        if text.trim().is_empty() {
            return None;
        }
        Some(text)
    }
}

/// Walk a dot-separated JSON path and stringify the leaf value.
///
/// Primitives are converted to text, objects/arrays are serialized as JSON.
fn extract_json_field(value: &serde_json::Value, path: &str) -> Option<String> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    match current {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        other @ (serde_json::Value::Array(_) | serde_json::Value::Object(_)) => {
            Some(other.to_string())
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::post;
    use tower::ServiceExt;

    use crate::config::WebhookRouting;

    async fn make_app(
        webhooks: HashMap<String, WebhookEndpointState>,
    ) -> (
        axum::Router,
        crate::bus::Subscriber<crate::bus::NotificationEvent>,
    ) {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let subscriber = bus_handle
            .subscribe(crate::bus::topics::Inbox)
            .await
            .unwrap();
        let state = WebhookState {
            publisher,
            webhooks,
        };
        let app = axum::Router::new()
            .route("/webhook/{name}", post(webhook_handler))
            .with_state(state);
        (app, subscriber)
    }

    fn parsed_endpoint(secret: Option<&str>) -> WebhookEndpointState {
        WebhookEndpointState {
            secret: secret.map(String::from),
            format: WebhookFormat::Parsed,
            content_fields: None,
            routing: WebhookRouting::default(),
        }
    }

    fn single_webhook(
        name: &str,
        endpoint: WebhookEndpointState,
    ) -> HashMap<String, WebhookEndpointState> {
        let mut map = HashMap::new();
        map.insert(name.to_string(), endpoint);
        map
    }

    #[tokio::test]
    async fn unknown_webhook_returns_404() {
        let (app, _sub) = make_app(HashMap::new()).await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/nonexistent")
            .body(Body::from("hello"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn json_payload_accepted() {
        let (app, mut sub) = make_app(single_webhook("test", parsed_endpoint(None))).await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/test")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"content":"hello from webhook"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let notification = tokio::time::timeout(std::time::Duration::from_millis(100), sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(notification.content, "hello from webhook");
        assert_eq!(notification.title, "webhook:test");
    }

    #[tokio::test]
    async fn plain_text_accepted() {
        let (app, mut sub) = make_app(single_webhook("test", parsed_endpoint(None))).await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/test")
            .body(Body::from("plain text message"))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let notification = tokio::time::timeout(std::time::Duration::from_millis(100), sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(notification.content, "plain text message");
    }

    #[tokio::test]
    async fn empty_body_rejected() {
        let (app, _sub) = make_app(single_webhook("test", parsed_endpoint(None))).await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/test")
            .body(Body::from(""))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn bearer_auth_required() {
        let (app, _sub) = make_app(single_webhook(
            "secure",
            parsed_endpoint(Some("secret-token")),
        ))
        .await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/secure")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_auth_wrong_token() {
        let (app, _sub) = make_app(single_webhook(
            "secure",
            parsed_endpoint(Some("secret-token")),
        ))
        .await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/secure")
            .header("content-type", "application/json")
            .header("authorization", "Bearer wrong-token")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_auth_correct_token() {
        let (app, mut sub) = make_app(single_webhook(
            "secure",
            parsed_endpoint(Some("secret-token")),
        ))
        .await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/secure")
            .header("content-type", "application/json")
            .header("authorization", "Bearer secret-token")
            .body(Body::from(r#"{"content":"hello"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let notification = tokio::time::timeout(std::time::Duration::from_millis(100), sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(notification.content, "hello");
    }

    #[tokio::test]
    async fn raw_format_passes_body() {
        let endpoint = WebhookEndpointState {
            secret: None,
            format: WebhookFormat::Raw,
            content_fields: None,
            routing: WebhookRouting::default(),
        };
        let (app, mut sub) = make_app(single_webhook("raw-hook", endpoint)).await;

        let body = r#"{"any":"json","or":"text"}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/webhook/raw-hook")
            .body(Body::from(body))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let notification = tokio::time::timeout(std::time::Duration::from_millis(100), sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(notification.content, body);
    }

    #[tokio::test]
    async fn parsed_with_content_fields() {
        let endpoint = WebhookEndpointState {
            secret: None,
            format: WebhookFormat::Parsed,
            content_fields: Some(vec!["issue.title".to_string(), "issue.body".to_string()]),
            routing: WebhookRouting::default(),
        };
        let (app, mut sub) = make_app(single_webhook("github", endpoint)).await;

        let json = r#"{"issue":{"title":"Bug report","body":"Something broke","labels":["bug"]}}"#;
        let req = Request::builder()
            .method("POST")
            .uri("/webhook/github")
            .header("content-type", "application/json")
            .body(Body::from(json))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let notification = tokio::time::timeout(std::time::Duration::from_millis(100), sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(notification.content, "Bug report\n\nSomething broke");
    }

    #[tokio::test]
    async fn parsed_content_fields_no_match_returns_400() {
        let endpoint = WebhookEndpointState {
            secret: None,
            format: WebhookFormat::Parsed,
            content_fields: Some(vec!["nonexistent.path".to_string()]),
            routing: WebhookRouting::default(),
        };
        let (app, _sub) = make_app(single_webhook("test", endpoint)).await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/test")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"other":"data"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn multiple_webhooks_different_auth() {
        let mut webhooks = HashMap::new();
        webhooks.insert("public".to_string(), parsed_endpoint(None));
        webhooks.insert("private".to_string(), parsed_endpoint(Some("tok123")));
        let (app, _sub) = make_app(webhooks).await;

        // Public should accept without auth
        let pub_req = Request::builder()
            .method("POST")
            .uri("/webhook/public")
            .body(Body::from("hello"))
            .unwrap();
        let pub_resp = app.clone().oneshot(pub_req).await.unwrap();
        assert_eq!(pub_resp.status(), StatusCode::ACCEPTED);

        // Private should reject without auth
        let priv_req = Request::builder()
            .method("POST")
            .uri("/webhook/private")
            .body(Body::from("hello"))
            .unwrap();
        let priv_resp = app.oneshot(priv_req).await.unwrap();
        assert_eq!(priv_resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_agent_routing_publishes_spawn_request() {
        let endpoint = WebhookEndpointState {
            secret: None,
            format: WebhookFormat::Parsed,
            content_fields: None,
            routing: WebhookRouting::Agent("code_reviewer".to_string()),
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut preset_sub = bus_handle
            .subscribe(crate::bus::topics::SpawnRequest(
                crate::bus::PresetName::from("code_reviewer"),
            ))
            .await
            .unwrap();

        let state = WebhookState {
            publisher,
            webhooks: single_webhook("agent-hook", endpoint),
        };
        let app = axum::Router::new()
            .route("/webhook/{name}", post(webhook_handler))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/agent-hook")
            .body(Body::from(r#"{"content":"review this"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let event = tokio::time::timeout(std::time::Duration::from_millis(100), preset_sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(event.prompt, "review this");
        assert_eq!(event.source_label, "webhook:agent-hook");
    }

    #[tokio::test]
    async fn webhook_inbox_routing_publishes_notification() {
        let endpoint = WebhookEndpointState {
            secret: None,
            format: WebhookFormat::Parsed,
            content_fields: None,
            routing: WebhookRouting::Inbox,
        };
        let (app, mut sub) = make_app(single_webhook("inbox-hook", endpoint)).await;

        let req = Request::builder()
            .method("POST")
            .uri("/webhook/inbox-hook")
            .body(Body::from(r#"{"content":"notify me"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let notification = tokio::time::timeout(std::time::Duration::from_millis(100), sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(notification.content, "notify me");
        assert_eq!(notification.title, "webhook:inbox-hook");
    }

    #[test]
    fn extract_json_field_dot_path() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"a":{"b":{"c":"deep"}},"x":42,"flag":true}"#).unwrap();

        assert_eq!(extract_json_field(&json, "a.b.c"), Some("deep".to_string()));
        assert_eq!(extract_json_field(&json, "x"), Some("42".to_string()));
        assert_eq!(extract_json_field(&json, "flag"), Some("true".to_string()));
        assert_eq!(extract_json_field(&json, "missing"), None);
        assert_eq!(extract_json_field(&json, "a.missing"), None);
    }

    #[test]
    fn extract_json_field_array_serialized() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"labels":["bug","urgent"]}"#).unwrap();
        let result = extract_json_field(&json, "labels").unwrap();
        assert_eq!(result, r#"["bug","urgent"]"#);
    }
}
