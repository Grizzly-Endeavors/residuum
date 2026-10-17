//! Decision model (System 1) endpoints for the settings form: list a
//! provider's models and test a configuration before it is saved.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::config::secrets::SecretStore;
use crate::config::{SystemOneConfig, SystemOneProvider};
use crate::inference::system_one::{Question, client_for_config};

use super::HubDir;

/// Body of `POST /api/hub/system-one/models` and `/test`: the form's values,
/// saved or not.
#[derive(Deserialize)]
pub(super) struct SystemOneFormRequest {
    provider: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    model: Option<String>,
    /// A literal key or a `secret:<name>` reference.
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    keep_alive: Option<String>,
}

#[derive(Serialize)]
pub(super) struct SystemOneModelEntry {
    id: String,
    description: Option<String>,
}

#[derive(Serialize)]
pub(super) struct SystemOneModelsResponse {
    models: Vec<SystemOneModelEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
pub(super) struct SystemOneTestResponse {
    ok: bool,
    /// What happened, in plain words.
    message: String,
    /// The versioned model that answered, on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    answered_by: Option<String>,
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Resolve a `secret:` reference against the hub's store; anything else
/// passes through.
async fn resolve_key(hub_dir: std::path::PathBuf, raw: Option<String>) -> Option<String> {
    let raw = raw?;
    let Some(name) = raw.strip_prefix("secret:").map(str::to_owned) else {
        return Some(raw);
    };
    crate::util::spawn_blocking_in_span(move || {
        SecretStore::load(&hub_dir)
            .ok()
            .and_then(|s| s.get(&name).map(String::from))
    })
    .await
    .ok()
    .flatten()
}

/// The config the form describes. `model_required` is false for listing
/// models, which needs no model yet.
async fn form_config(
    hub_dir: std::path::PathBuf,
    req: SystemOneFormRequest,
    model_required: bool,
) -> Result<SystemOneConfig, String> {
    let provider: SystemOneProvider = req.provider.parse()?;
    let url = non_empty(req.url.as_deref())
        .or_else(|| provider.default_url().map(str::to_string))
        .ok_or("Enter the address of the decision model service.")?;
    let model =
        non_empty(req.model.as_deref()).or_else(|| provider.default_model().map(str::to_string));
    let model = match model {
        Some(m) => m,
        None if model_required => return Err("Choose a model first.".to_string()),
        None => String::new(),
    };
    let api_key = resolve_key(hub_dir, non_empty(req.api_key.as_deref())).await;
    Ok(SystemOneConfig {
        provider,
        url,
        model,
        api_key,
        keep_alive: non_empty(req.keep_alive.as_deref()),
    })
}

/// `POST /api/hub/system-one/models` — the model names the provider accepts.
pub(super) async fn api_system_one_models(
    State(HubDir(hub_dir)): State<HubDir>,
    Json(req): Json<SystemOneFormRequest>,
) -> Json<SystemOneModelsResponse> {
    let failed = |error: String| {
        Json(SystemOneModelsResponse {
            models: Vec::new(),
            error: Some(error),
        })
    };
    let cfg = match form_config(hub_dir, req, false).await {
        Ok(cfg) => cfg,
        Err(e) => return failed(e),
    };
    let client = match client_for_config(&cfg) {
        Ok(c) => c,
        Err(e) => return failed(e),
    };
    match client.list_models().await {
        Ok(models) => Json(SystemOneModelsResponse {
            models: models
                .into_iter()
                .map(|m| SystemOneModelEntry {
                    id: m.name,
                    description: m.description,
                })
                .collect(),
            error: None,
        }),
        Err(e) => {
            tracing::warn!(error = %e, "failed to list system one models");
            failed(e.user_message())
        }
    }
}

/// `POST /api/hub/system-one/test` — ask the configured model one small
/// question, without touching the running service or its status.
pub(super) async fn api_system_one_test(
    State(HubDir(hub_dir)): State<HubDir>,
    Json(req): Json<SystemOneFormRequest>,
) -> Json<SystemOneTestResponse> {
    let failed = |message: String| {
        Json(SystemOneTestResponse {
            ok: false,
            message,
            answered_by: None,
        })
    };
    let cfg = match form_config(hub_dir, req, true).await {
        Ok(cfg) => cfg,
        Err(e) => return failed(e),
    };
    let client = match client_for_config(&cfg) {
        Ok(c) => c,
        Err(e) => return failed(e),
    };
    let questions = BTreeMap::from([(
        "greeting".to_string(),
        Question::noul("Is this text a greeting?"),
    )]);
    match client
        .evaluate(
            &serde_json::Value::String("Hello there!".to_string()),
            &questions,
        )
        .await
    {
        Ok(response) => Json(SystemOneTestResponse {
            ok: true,
            message: format!(
                "{} answered. Decisions will use {}.",
                cfg.display_name(),
                cfg.model
            ),
            answered_by: Some(response.model),
        }),
        Err(e) => {
            tracing::warn!(error = %e, "system one connection test failed");
            failed(e.user_message())
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};
    use tower::ServiceExt as _;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn router(hub_dir: &std::path::Path) -> axum::Router {
        axum::Router::new()
            .route("/models", axum::routing::post(api_system_one_models))
            .route("/test", axum::routing::post(api_system_one_test))
            .with_state(HubDir(hub_dir.to_path_buf()))
    }

    async fn post(app: axum::Router, uri: &str, body: Value) -> Value {
        let response = app
            .oneshot(
                Request::post(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn models_resolves_a_saved_key_and_lists_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SecretStore::load(dir.path()).unwrap();
        store.set("typesafe", "sk-saved", dir.path()).unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sk-saved"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models": [{ "name": "jev-latest", "description": "flagship" }]
            })))
            .mount(&server)
            .await;

        let body = post(
            router(dir.path()),
            "/models",
            json!({ "provider": "typesafe", "url": server.uri(), "api_key": "secret:typesafe" }),
        )
        .await;
        assert_eq!(body.pointer("/models/0/id"), Some(&json!("jev-latest")));
        assert_eq!(body.get("error"), None);
    }

    #[tokio::test]
    async fn test_reports_success_and_the_answering_model() {
        let dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nimble",
                "answers": { "greeting": { "type": "noul", "noul": 0.99 } }
            })))
            .mount(&server)
            .await;
        let body = post(
            router(dir.path()),
            "/test",
            json!({ "provider": "ollama", "url": server.uri(), "model": "nimble" }),
        )
        .await;
        assert_eq!(body.get("ok"), Some(&json!(true)));
        assert_eq!(body.get("answered_by"), Some(&json!("nimble")));
    }

    #[tokio::test]
    async fn test_explains_a_missing_model_and_a_bad_key() {
        let dir = tempfile::tempdir().unwrap();
        let no_model = post(router(dir.path()), "/test", json!({ "provider": "ollama" })).await;
        assert_eq!(no_model.get("ok"), Some(&json!(false)));
        assert_eq!(
            no_model.get("message"),
            Some(&json!("Choose a model first."))
        );

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let body = post(
            router(dir.path()),
            "/test",
            json!({ "provider": "typesafe", "url": server.uri(), "api_key": "wrong" }),
        )
        .await;
        assert_eq!(body.get("ok"), Some(&json!(false)));
        assert!(
            body.get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains("API key")),
            "{body}"
        );
    }
}
