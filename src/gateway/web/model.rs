//! Model call API: `POST /api/model/complete`, a one-shot request/response
//! call to the background `small` model on behalf of an artifact (design
//! §7). No agent, no tools, no memory, no identity files — the model sees
//! only what the caller sends.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};

use crate::background::spawn_context::SpawnContext;
use crate::config::{BackgroundModelTier, BackgroundModelsConfig, ProviderSpec, RoleOverrides};
use crate::inference::retry::RetryConfig;
use crate::inference::{
    CompletionOptions, ImageData, InferenceError, InferenceProvider, Message, ResponseFormat, Role,
    SharedHttpClient, ThinkingConfig, Usage, build_provider_chain,
};

/// Header the bridge stamps on every relayed request; identifies the
/// artifact a model call is on behalf of.
const ARTIFACT_HEADER: &str = "X-Residuum-Artifact";

/// Identity label logged and reported when a call carries no artifact
/// header (a direct `web-ui` caller, not relayed through an artifact frame).
const WEB_UI_IDENTITY: &str = "web-ui";

/// The subset of `SpawnContext` `POST /api/model/complete` needs: enough to
/// resolve the small tier's provider chain and its `bg_small` overrides,
/// kept separate from `SpawnContext` so the HTTP layer doesn't carry every
/// session-fork dependency (session registries, MCP, action store, ...)
/// that type also holds.
pub(crate) struct ModelCallResources {
    background_models: BackgroundModelsConfig,
    main_provider_specs: Vec<ProviderSpec>,
    http_client: SharedHttpClient,
    max_tokens: u32,
    retry_config: RetryConfig,
    role_overrides: HashMap<String, RoleOverrides>,
    default_temperature: Option<f32>,
    default_thinking: Option<ThinkingConfig>,
}

impl ModelCallResources {
    /// Build from the current `SpawnContext`, which the gateway rebuilds
    /// fresh on every config reload (see `crate::gateway::reload`) — a
    /// `ModelCallResources` built here reflects the config active at the
    /// moment it is called.
    pub(crate) fn from_spawn_context(ctx: &SpawnContext) -> Self {
        Self {
            background_models: ctx.background_config.models.clone(),
            main_provider_specs: ctx.main_provider_specs.clone(),
            http_client: ctx.http_client.clone(),
            max_tokens: ctx.max_tokens,
            retry_config: ctx.retry_config.clone(),
            role_overrides: ctx.role_overrides.clone(),
            default_temperature: ctx.options.temperature,
            default_thinking: ctx.options.thinking.clone(),
        }
    }

    /// The small tier's resolved provider chain: small → medium → large →
    /// main, per `BackgroundModelsConfig::resolve_tier`.
    fn small_specs(&self) -> Vec<ProviderSpec> {
        self.background_models
            .resolve_tier(&BackgroundModelTier::Small, &self.main_provider_specs)
    }

    /// The `bg_small` role override, if configured.
    fn bg_small_override(&self) -> Option<&RoleOverrides> {
        self.role_overrides.get("bg_small")
    }
}

/// Shared state for the model call API. `resources` is a `watch` receiver so
/// every request reads whatever `ModelCallResources` the gateway last built
/// on reload, without the HTTP router needing to be rebuilt.
#[derive(Clone)]
pub(crate) struct ModelApiState {
    pub(crate) resources: tokio::sync::watch::Receiver<Arc<ModelCallResources>>,
}

pub(crate) fn model_api_router(state: ModelApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/model/complete", post(api_model_complete))
        .with_state(state)
}

/// One message in a `POST /api/model/complete` request.
#[derive(Deserialize)]
struct RequestMessage {
    role: String,
    content: String,
    #[serde(default)]
    images: Vec<ImageData>,
}

/// Body for `POST /api/model/complete` (design §7): either `{ "prompt" }`
/// shorthand for one user message, or `system`/`messages` for a full
/// conversation, plus optional `schema`, `max_tokens`, and `temperature`.
#[derive(Deserialize)]
pub(super) struct ModelCompleteRequest {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    messages: Option<Vec<RequestMessage>>,
    #[serde(default)]
    schema: Option<serde_json::Value>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    temperature: Option<f32>,
}

/// Token usage reported back to the caller.
#[derive(Serialize, Default)]
#[cfg_attr(test, derive(serde::Deserialize))]
struct UsageResponse {
    input_tokens: u32,
    output_tokens: u32,
}

impl From<Usage> for UsageResponse {
    fn from(usage: Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        }
    }
}

/// Response from `POST /api/model/complete`.
#[derive(Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
struct ModelCompleteResponse {
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    json: Option<serde_json::Value>,
    model: String,
    usage: UsageResponse,
}

/// `{ "error": "..." }` body for a non-2xx response.
#[derive(Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
struct ErrorBody {
    error: String,
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
        .into_response()
}

/// The artifact identity from the bridge-stamped header, or `None` for a
/// caller that didn't carry one (only the web UI's own bridge sets it;
/// nothing else can reach this route through the cross-site guard).
fn artifact_identity(headers: &HeaderMap) -> Option<String> {
    headers
        .get(ARTIFACT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Defaults applied when the request doesn't override them: the small
/// tier's `max_tokens`, and `bg_small`'s temperature/thinking (falling back
/// to the global default when `bg_small` doesn't set them).
struct RequestDefaults {
    max_tokens: u32,
    temperature: Option<f32>,
    thinking: Option<ThinkingConfig>,
}

impl RequestDefaults {
    fn from_resources(resources: &ModelCallResources) -> Self {
        let bg_small = resources.bg_small_override();
        Self {
            max_tokens: resources.max_tokens,
            temperature: bg_small
                .and_then(|o| o.temperature)
                .or(resources.default_temperature),
            thinking: bg_small
                .and_then(|o| o.thinking.clone())
                .or_else(|| resources.default_thinking.clone()),
        }
    }
}

/// Build the messages and completion options for one call, applying the
/// design's override rule: the request's own `temperature`/`max_tokens` win
/// when given, otherwise the small tier's defaults apply.
///
/// # Errors
/// Returns a plain-language message for a malformed request: no
/// `prompt`/`messages`, an unrecognized message role, or empty content.
fn build_request(
    req: &ModelCompleteRequest,
    defaults: &RequestDefaults,
) -> Result<(Vec<Message>, CompletionOptions, bool), String> {
    let mut messages = Vec::new();
    if let Some(system) = req.system.as_deref()
        && !system.trim().is_empty()
    {
        messages.push(Message::system(system));
    }

    if let Some(raw) = &req.messages {
        if raw.is_empty() {
            return Err("\"messages\" must not be empty".to_string());
        }
        for m in raw {
            let role = match m.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                other => {
                    return Err(format!(
                        "message role must be \"user\" or \"assistant\", got \"{other}\""
                    ));
                }
            };
            if m.content.trim().is_empty() {
                return Err("message content must not be empty".to_string());
            }
            messages.push(Message {
                role,
                content: m.content.clone(),
                tool_calls: None,
                tool_call_id: None,
                images: m.images.clone(),
                sender: None,
                agent_sender: None,
            });
        }
    } else {
        let prompt = req.prompt.as_deref().unwrap_or_default();
        if prompt.trim().is_empty() {
            return Err("request needs a non-empty \"prompt\" or \"messages\"".to_string());
        }
        messages.push(Message::user(prompt));
    }

    let schema_requested = req.schema.is_some();
    let response_format = match &req.schema {
        Some(schema) => ResponseFormat::JsonSchema {
            name: "artifact_response".to_string(),
            schema: schema.clone(),
        },
        None => ResponseFormat::Text,
    };

    let options = CompletionOptions {
        max_tokens: Some(req.max_tokens.unwrap_or(defaults.max_tokens)),
        temperature: req.temperature.or(defaults.temperature),
        thinking: defaults.thinking.clone(),
        response_format,
        web_search: None,
    };

    Ok((messages, options, schema_requested))
}

/// Run the completion against an already-resolved provider and map the
/// result to the design's response/error contract (§7): `json` parsed from
/// the model's content when a schema was requested (`502` if it doesn't
/// parse), `504` on provider timeout, `502` with a plain message for any
/// other provider failure (the raw error goes to logs only).
async fn run_completion(
    provider: &dyn InferenceProvider,
    model_label: &str,
    identity_label: &str,
    messages: &[Message],
    options: &CompletionOptions,
    schema_requested: bool,
) -> Response {
    match provider.complete(messages, &[], options).await {
        Ok(resp) => {
            let json = if schema_requested {
                let content = crate::memory::strip_code_fences(resp.content.trim());
                match serde_json::from_str::<serde_json::Value>(content) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            artifact = identity_label,
                            model = model_label,
                            "model call requested structured output but returned unparsable content"
                        );
                        return error_response(
                            StatusCode::BAD_GATEWAY,
                            "The model didn't return valid structured output for the requested schema.",
                        );
                    }
                }
            } else {
                None
            };
            let usage = resp.usage.unwrap_or_default();
            tracing::info!(
                artifact = identity_label,
                model = model_label,
                input_tokens = usage.input_tokens,
                output_tokens = usage.output_tokens,
                "model call completed"
            );
            Json(ModelCompleteResponse {
                content: resp.content,
                json,
                model: model_label.to_string(),
                usage: UsageResponse::from(usage),
            })
            .into_response()
        }
        Err(InferenceError::Timeout(secs)) => {
            tracing::warn!(
                artifact = identity_label,
                model = model_label,
                timeout_secs = secs,
                "model call timed out"
            );
            error_response(
                StatusCode::GATEWAY_TIMEOUT,
                "The model didn't respond in time. Try again in a moment.",
            )
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                artifact = identity_label,
                model = model_label,
                "model call failed"
            );
            error_response(
                StatusCode::BAD_GATEWAY,
                "The model provider failed to respond. Try again in a moment.",
            )
        }
    }
}

/// `POST /api/model/complete` — a one-shot small-model call on an
/// artifact's behalf (design §7).
pub(super) async fn api_model_complete(
    State(state): State<ModelApiState>,
    headers: HeaderMap,
    Json(req): Json<ModelCompleteRequest>,
) -> Response {
    let identity_label = artifact_identity(&headers).unwrap_or_else(|| WEB_UI_IDENTITY.to_string());
    let resources = Arc::clone(&state.resources.borrow());

    let defaults = RequestDefaults::from_resources(&resources);
    let (messages, options, schema_requested) = match build_request(&req, &defaults) {
        Ok(built) => built,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let specs = resources.small_specs();
    let Some(primary) = specs.first() else {
        tracing::error!(
            artifact = %identity_label,
            "no model configured to answer a model call (small tier and main both empty)"
        );
        return error_response(
            StatusCode::BAD_GATEWAY,
            "No model is configured to answer this call. Check your provider settings.",
        );
    };
    let model_label = primary.model.to_string();

    let provider = match build_provider_chain(
        &specs,
        resources.max_tokens,
        resources.http_client.clone(),
        resources.retry_config.clone(),
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(
                error = %e,
                artifact = %identity_label,
                "failed to build model provider for artifact call"
            );
            return error_response(
                StatusCode::BAD_GATEWAY,
                "Couldn't reach the model provider. Check your provider configuration.",
            );
        }
    };

    run_completion(
        provider.as_ref(),
        &model_label,
        &identity_label,
        &messages,
        &options,
        schema_requested,
    )
    .await
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    use crate::inference::{HttpClientConfig, InferenceResponse, ToolDefinition};

    // ── artifact_identity ────────────────────────────────────────────

    #[test]
    fn artifact_identity_reads_header() {
        let mut headers = HeaderMap::new();
        headers.insert(ARTIFACT_HEADER, "chart".parse().unwrap());
        assert_eq!(artifact_identity(&headers).as_deref(), Some("chart"));
    }

    #[test]
    fn artifact_identity_none_when_absent_or_blank() {
        assert_eq!(artifact_identity(&HeaderMap::new()), None);

        let mut headers = HeaderMap::new();
        headers.insert(ARTIFACT_HEADER, "   ".parse().unwrap());
        assert_eq!(artifact_identity(&headers), None);
    }

    // ── build_request ────────────────────────────────────────────────

    fn defaults() -> RequestDefaults {
        RequestDefaults {
            max_tokens: 512,
            temperature: Some(0.4),
            thinking: None,
        }
    }

    #[test]
    fn build_request_shorthand_prompt_makes_one_user_message() {
        let req = ModelCompleteRequest {
            prompt: Some("summarize this".to_string()),
            system: None,
            messages: None,
            schema: None,
            max_tokens: None,
            temperature: None,
        };
        let (messages, options, schema_requested) = build_request(&req, &defaults()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[0].content, "summarize this");
        assert!(!schema_requested);
        assert_eq!(options.max_tokens, Some(512));
        assert_eq!(options.temperature, Some(0.4));
        assert!(matches!(options.response_format, ResponseFormat::Text));
    }

    #[test]
    fn build_request_full_form_with_system_and_images() {
        let req = ModelCompleteRequest {
            prompt: None,
            system: Some("be terse".to_string()),
            messages: Some(vec![RequestMessage {
                role: "user".to_string(),
                content: "what's in this?".to_string(),
                images: vec![ImageData {
                    media_type: "image/png".to_string(),
                    data: "base64==".to_string(),
                }],
            }]),
            schema: None,
            max_tokens: Some(64),
            temperature: Some(0.9),
        };
        let (messages, options, _) = build_request(&req, &defaults()).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content, "be terse");
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].images.len(), 1);
        // Request-level max_tokens/temperature win over the defaults.
        assert_eq!(options.max_tokens, Some(64));
        assert_eq!(options.temperature, Some(0.9));
    }

    #[test]
    fn build_request_schema_produces_json_schema_format() {
        let req = ModelCompleteRequest {
            prompt: Some("classify".to_string()),
            system: None,
            messages: None,
            schema: Some(serde_json::json!({"type": "object"})),
            max_tokens: None,
            temperature: None,
        };
        let (_, options, schema_requested) = build_request(&req, &defaults()).unwrap();
        assert!(schema_requested);
        assert!(matches!(
            options.response_format,
            ResponseFormat::JsonSchema { .. }
        ));
    }

    #[test]
    fn build_request_rejects_missing_prompt_and_messages() {
        let req = ModelCompleteRequest {
            prompt: None,
            system: None,
            messages: None,
            schema: None,
            max_tokens: None,
            temperature: None,
        };
        assert!(build_request(&req, &defaults()).is_err());
    }

    #[test]
    fn build_request_rejects_blank_prompt() {
        let req = ModelCompleteRequest {
            prompt: Some("   ".to_string()),
            system: None,
            messages: None,
            schema: None,
            max_tokens: None,
            temperature: None,
        };
        assert!(build_request(&req, &defaults()).is_err());
    }

    #[test]
    fn build_request_rejects_bad_role() {
        let req = ModelCompleteRequest {
            prompt: None,
            system: None,
            messages: Some(vec![RequestMessage {
                role: "system".to_string(),
                content: "hi".to_string(),
                images: vec![],
            }]),
            schema: None,
            max_tokens: None,
            temperature: None,
        };
        let err = build_request(&req, &defaults()).unwrap_err();
        assert!(err.contains("role"));
    }

    #[test]
    fn build_request_rejects_empty_message_content() {
        let req = ModelCompleteRequest {
            prompt: None,
            system: None,
            messages: Some(vec![RequestMessage {
                role: "user".to_string(),
                content: "  ".to_string(),
                images: vec![],
            }]),
            schema: None,
            max_tokens: None,
            temperature: None,
        };
        assert!(build_request(&req, &defaults()).is_err());
    }

    // ── run_completion ───────────────────────────────────────────────

    struct StubProvider {
        result: Mutex<Option<Result<InferenceResponse, InferenceError>>>,
    }

    impl StubProvider {
        fn once(result: Result<InferenceResponse, InferenceError>) -> Self {
            Self {
                result: Mutex::new(Some(result)),
            }
        }
    }

    #[async_trait]
    impl InferenceProvider for StubProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            self.result
                .lock()
                .expect("stub lock poisoned")
                .take()
                .expect("stub provider called more than once")
        }

        fn model_name(&self) -> &'static str {
            "stub"
        }
    }

    fn ok_response(content: &str, usage: Usage) -> InferenceResponse {
        InferenceResponse {
            content: content.to_string(),
            tool_calls: vec![],
            usage: Some(usage),
            thinking: None,
        }
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn run_completion_success_plain_text() {
        let provider = StubProvider::once(Ok(ok_response(
            "hello there",
            Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        )));
        let messages = vec![Message::user("hi")];
        let resp = run_completion(
            &provider,
            "anthropic/claude-test",
            "chart",
            &messages,
            &CompletionOptions::default(),
            false,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed: ModelCompleteResponse = serde_json::from_value(body_json(resp).await).unwrap();
        assert_eq!(parsed.content, "hello there");
        assert!(parsed.json.is_none());
        assert_eq!(parsed.model, "anthropic/claude-test");
        assert_eq!(parsed.usage.input_tokens, 10);
        assert_eq!(parsed.usage.output_tokens, 5);
    }

    #[tokio::test]
    async fn run_completion_schema_parses_json_content() {
        let provider = StubProvider::once(Ok(ok_response(r#"{"answer": 42}"#, Usage::default())));
        let messages = vec![Message::user("classify")];
        let resp = run_completion(
            &provider,
            "anthropic/claude-test",
            "chart",
            &messages,
            &CompletionOptions::default(),
            true,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed: ModelCompleteResponse = serde_json::from_value(body_json(resp).await).unwrap();
        assert_eq!(parsed.json, Some(serde_json::json!({"answer": 42})));
    }

    #[tokio::test]
    async fn run_completion_unparsable_structured_output_is_502() {
        let provider = StubProvider::once(Ok(ok_response("not json at all", Usage::default())));
        let messages = vec![Message::user("classify")];
        let resp = run_completion(
            &provider,
            "anthropic/claude-test",
            "chart",
            &messages,
            &CompletionOptions::default(),
            true,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = body_json(resp).await;
        assert!(body["error"].as_str().unwrap().contains("structured"));
    }

    #[tokio::test]
    async fn run_completion_provider_failure_is_502_with_plain_message() {
        let provider = StubProvider::once(Err(InferenceError::Api(
            "super secret upstream stack trace".to_string(),
        )));
        let messages = vec![Message::user("hi")];
        let resp = run_completion(
            &provider,
            "anthropic/claude-test",
            "chart",
            &messages,
            &CompletionOptions::default(),
            false,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = body_json(resp).await;
        let message = body["error"].as_str().unwrap();
        assert!(!message.contains("super secret"));
    }

    #[tokio::test]
    async fn run_completion_timeout_is_504() {
        let provider = StubProvider::once(Err(InferenceError::Timeout(30)));
        let messages = vec![Message::user("hi")];
        let resp = run_completion(
            &provider,
            "anthropic/claude-test",
            "chart",
            &messages,
            &CompletionOptions::default(),
            false,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
    }

    // ── small-tier resolution and reload-following ──────────────────

    fn spec(name: &str) -> ProviderSpec {
        ProviderSpec {
            name: name.to_string(),
            model: crate::config::ModelSpec {
                kind: crate::config::ProviderKind::Ollama,
                model: name.to_string(),
            },
            provider_url: "http://localhost:11434".to_string(),
            api_key: None,
            keep_alive: None,
            session_affinity: None,
        }
    }

    fn resources_with(
        models: BackgroundModelsConfig,
        main: Vec<ProviderSpec>,
    ) -> ModelCallResources {
        ModelCallResources {
            background_models: models,
            main_provider_specs: main,
            http_client: SharedHttpClient::new(&HttpClientConfig::default()).unwrap(),
            max_tokens: 1024,
            retry_config: RetryConfig::no_retry(),
            role_overrides: HashMap::new(),
            default_temperature: None,
            default_thinking: None,
        }
    }

    #[test]
    fn small_specs_uses_configured_small_tier() {
        let resources = resources_with(
            BackgroundModelsConfig {
                small: Some(vec![spec("small-model")]),
                medium: Some(vec![spec("medium-model")]),
                large: None,
            },
            vec![spec("main-model")],
        );
        let specs = resources.small_specs();
        assert_eq!(specs.first().unwrap().model.model, "small-model");
    }

    #[test]
    fn small_specs_falls_back_through_medium_and_large_to_main() {
        let all_unset = resources_with(BackgroundModelsConfig::default(), vec![spec("main-model")]);
        let specs = all_unset.small_specs();
        assert_eq!(specs.first().unwrap().model.model, "main-model");

        let medium_only = resources_with(
            BackgroundModelsConfig {
                small: None,
                medium: Some(vec![spec("medium-model")]),
                large: None,
            },
            vec![spec("main-model")],
        );
        assert_eq!(
            medium_only.small_specs().first().unwrap().model.model,
            "medium-model"
        );
    }

    #[tokio::test]
    async fn model_api_state_follows_reload_pushed_resources() {
        let initial = Arc::new(resources_with(
            BackgroundModelsConfig {
                small: Some(vec![spec("old-model")]),
                medium: None,
                large: None,
            },
            vec![spec("main-model")],
        ));
        let (tx, rx) = tokio::sync::watch::channel(initial);
        let state = ModelApiState { resources: rx };

        let before = Arc::clone(&state.resources.borrow());
        assert_eq!(
            before.small_specs().first().unwrap().model.model,
            "old-model"
        );

        // Simulate a config reload rebuilding resources and pushing them,
        // the same way `crate::gateway::reload` does on every root reload.
        let updated = Arc::new(resources_with(
            BackgroundModelsConfig {
                small: Some(vec![spec("new-model")]),
                medium: None,
                large: None,
            },
            vec![spec("main-model")],
        ));
        tx.send(updated).unwrap();

        let after = Arc::clone(&state.resources.borrow());
        assert_eq!(
            after.small_specs().first().unwrap().model.model,
            "new-model"
        );
    }

    #[test]
    fn bg_small_override_applies_temperature_and_thinking_over_global_default() {
        let mut role_overrides = HashMap::new();
        role_overrides.insert(
            "bg_small".to_string(),
            RoleOverrides {
                temperature: Some(0.1),
                thinking: Some(ThinkingConfig::Toggle(true)),
            },
        );
        let mut resources = resources_with(BackgroundModelsConfig::default(), vec![spec("main")]);
        resources.role_overrides = role_overrides;
        resources.default_temperature = Some(0.7);

        let defaults = RequestDefaults::from_resources(&resources);
        assert_eq!(defaults.temperature, Some(0.1));
        assert_eq!(defaults.thinking, Some(ThinkingConfig::Toggle(true)));
    }

    #[test]
    fn bg_small_override_falls_back_to_global_default_when_unset() {
        let mut resources = resources_with(BackgroundModelsConfig::default(), vec![spec("main")]);
        resources.default_temperature = Some(0.55);

        let defaults = RequestDefaults::from_resources(&resources);
        assert_eq!(defaults.temperature, Some(0.55));
        assert_eq!(defaults.thinking, None);
    }
}
