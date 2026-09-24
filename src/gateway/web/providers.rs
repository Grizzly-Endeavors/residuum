//! Provider model listing endpoints and types.

use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{Json, Response};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::config::secrets::SecretStore;
use crate::inference::providers::anthropic::is_oauth_key;

use super::ConfigApiState;
use super::config::ValidateResponse;

/// Request body for `POST /api/providers/models`.
#[derive(Deserialize)]
pub(super) struct ModelsRequest {
    provider: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// A single model entry returned by the listing endpoint.
#[derive(Serialize)]
pub(super) struct ModelEntry {
    id: String,
    name: String,
}

/// Response from the model listing endpoint.
#[derive(Serialize)]
pub(super) struct ModelsResponse {
    models: Vec<ModelEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `POST /api/providers/models` — fetch available models from a provider API.
///
/// Used by the setup wizard and settings page to populate model dropdowns.
/// Takes provider type, optional API key, and optional base URL.
pub(super) async fn api_provider_models(
    State(state): State<ConfigApiState>,
    Json(req): Json<ModelsRequest>,
) -> Json<ModelsResponse> {
    // Resolve secret: prefixed API keys via the encrypted store
    let resolved_key = if let Some(name) = req
        .api_key
        .as_deref()
        .and_then(|raw| raw.strip_prefix("secret:"))
    {
        let dir = state.config_dir.clone();
        let name_owned = name.to_owned();
        tokio::task::spawn_blocking(move || -> Option<String> {
            SecretStore::load(&dir)
                .ok()
                .and_then(|s| s.get(&name_owned).map(String::from))
        })
        .await
        .ok()
        .flatten()
    } else {
        req.api_key
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return Json(ModelsResponse {
                models: Vec::new(),
                error: Some(format!("failed to build HTTP client: {e}")),
            });
        }
    };

    let result = match req.provider.as_str() {
        "anthropic" => {
            fetch_anthropic_models(&client, resolved_key.as_deref(), req.url.as_deref()).await
        }
        "openai" => fetch_openai_models(&client, resolved_key.as_deref(), req.url.as_deref()).await,
        "gemini" => fetch_gemini_models(&client, resolved_key.as_deref(), req.url.as_deref()).await,
        "fireworks" => {
            fetch_fireworks_models(&client, resolved_key.as_deref(), req.url.as_deref()).await
        }
        "ollama" => fetch_ollama_models(&client, req.url.as_deref()).await,
        other => Err(format!("unknown provider: {other}")),
    };

    match result {
        Ok(mut models) => {
            models.sort_by(|a, b| a.id.cmp(&b.id));
            Json(ModelsResponse {
                models,
                error: None,
            })
        }
        Err(err) => Json(ModelsResponse {
            models: Vec::new(),
            error: Some(err),
        }),
    }
}

/// Fetch models from Anthropic's `/v1/models` endpoint.
async fn fetch_anthropic_models(
    client: &reqwest::Client,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let key = api_key.ok_or("api_key is required for anthropic")?;
    let base = base_url.unwrap_or("https://api.anthropic.com");
    let url = format!("{base}/v1/models?limit=1000");

    let mut req_builder = client.get(&url).header("anthropic-version", "2023-06-01");

    // OAuth tokens use Bearer auth + beta header; standard keys use x-api-key.
    if is_oauth_key(key) {
        req_builder = req_builder
            .header("Authorization", format!("Bearer {key}"))
            .header(
                "anthropic-beta",
                crate::inference::providers::anthropic::OAUTH_BETA,
            )
            .header(
                "user-agent",
                crate::inference::providers::anthropic::OAUTH_USER_AGENT,
            )
            .header("x-app", "cli");
    } else {
        req_builder = req_builder.header("X-Api-Key", key);
    }

    let resp = req_builder
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("anthropic returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    let data = json
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("missing data array")?;

    Ok(data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let name = m
                .get("display_name")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| m.get("id").and_then(|v| v.as_str()).unwrap_or(""))
                .to_string();
            Some(ModelEntry { id, name })
        })
        .collect())
}

/// Fetch models from the `OpenAI` `/models` endpoint.
async fn fetch_openai_models(
    client: &reqwest::Client,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let key = api_key.ok_or("api_key is required for openai")?;
    let base = base_url.unwrap_or("https://api.openai.com/v1");
    let url = format!("{base}/models");

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("openai returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    parse_openai_models(&json)
}

/// List every model the endpoint reports, fine-tunes included.
fn parse_openai_models(json: &serde_json::Value) -> Result<Vec<ModelEntry>, String> {
    let data = json
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("missing data array")?;

    Ok(data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            Some(ModelEntry {
                id: id.to_string(),
                name: id.to_string(),
            })
        })
        .collect())
}

/// Fetch chat-capable models from the Fireworks `/models` endpoint.
async fn fetch_fireworks_models(
    client: &reqwest::Client,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let key = api_key.ok_or("api_key is required for fireworks")?;
    let base = base_url.unwrap_or("https://api.fireworks.ai/inference/v1");
    let url = format!("{base}/models");

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("fireworks returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    parse_fireworks_chat_models(&json)
}

/// Keep the models that can drive an agent turn.
///
/// Fireworks lists embedding and reranker models alongside chat models (and
/// marks even those `supports_chat`), so the `kind` field is what separates them.
fn parse_fireworks_chat_models(json: &serde_json::Value) -> Result<Vec<ModelEntry>, String> {
    let data = json
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("missing data array")?;

    Ok(data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            let is_embedding = m.get("kind").and_then(|v| v.as_str()) == Some("EMBEDDING_MODEL");
            let supports_tools = m
                .get("supports_tools")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            if is_embedding || !supports_tools {
                return None;
            }
            Some(ModelEntry {
                id: id.to_string(),
                name: id.to_string(),
            })
        })
        .collect())
}

/// Fetch models from Google Gemini's `/models` endpoint.
async fn fetch_gemini_models(
    client: &reqwest::Client,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let key = api_key.ok_or("api_key is required for gemini")?;
    let base = base_url.unwrap_or("https://generativelanguage.googleapis.com/v1beta");
    let url = format!("{base}/models?key={key}&pageSize=1000");

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("gemini returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    let models = json
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or("missing models array")?;

    Ok(models
        .iter()
        .filter_map(|m| {
            // Only include models that support generateContent
            let methods = m
                .get("supportedGenerationMethods")
                .and_then(|v| v.as_array())?;
            let supports_generate = methods
                .iter()
                .any(|method| method.as_str().is_some_and(|s| s == "generateContent"));
            if !supports_generate {
                return None;
            }

            let raw_name = m.get("name")?.as_str()?;
            let id = raw_name
                .strip_prefix("models/")
                .unwrap_or(raw_name)
                .to_string();
            let display = m
                .get("displayName")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            Some(ModelEntry { id, name: display })
        })
        .collect())
}

/// Fetch models from Ollama's `/api/tags` endpoint.
async fn fetch_ollama_models(
    client: &reqwest::Client,
    base_url: Option<&str>,
) -> Result<Vec<ModelEntry>, String> {
    let base = base_url.unwrap_or("http://localhost:11434");
    let url = format!("{base}/api/tags");

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("ollama returned {status}: {body}"));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("invalid json: {err}"))?;
    let models = json
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or("missing models array")?;

    Ok(models
        .iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?.to_string();
            Some(ModelEntry {
                id: name.clone(),
                name,
            })
        })
        .collect())
}

/// `GET /api/providers/raw` — return raw `providers.toml` contents as text.
pub(super) async fn api_providers_raw_get(
    State(state): State<ConfigApiState>,
) -> Result<Response, (StatusCode, String)> {
    let providers_path = state.config_dir.join("providers.toml");
    let contents = tokio::fs::read_to_string(&providers_path)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read providers.toml: {e}"),
            )
        })?;
    Response::builder()
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(contents))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("response build error: {e}"),
            )
        })
}

/// `PUT /api/providers/raw` — validate and write `providers.toml`, trigger reload.
pub(super) async fn api_providers_raw_put(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    if let Err(e) = Config::validate_providers_toml(&body, &state.config_dir) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(e),
            }),
        ));
    }

    let providers_path = state.config_dir.join("providers.toml");
    tokio::fs::write(&providers_path, &body)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write providers.toml: {e}")),
                }),
            )
        })?;

    // Trigger root reload — provider changes affect model resolution
    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Root).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

/// `PATCH /api/providers/patch` — merge a JSON diff into the existing
/// `providers.toml`, validate, save, trigger reload if running.
///
/// The diff's shape mirrors `providers.toml`'s section/key layout, carrying
/// only the fields the Settings form actually changed — see
/// `crate::config::patch` for the exact convention. Model-role assignments
/// that carry `temperature`/`thinking` overrides use the `{"$inline": {...}}`
/// marker to become a TOML inline table; a plain string replaces the whole
/// role assignment.
pub(super) async fn api_providers_patch(
    State(state): State<ConfigApiState>,
    Json(diff): Json<serde_json::Value>,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    let bad_request = |msg: String| {
        (
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(msg),
            }),
        )
    };

    let Some(diff_map) = diff.as_object() else {
        return Err(bad_request(
            "providers patch must be a JSON object".to_string(),
        ));
    };

    let providers_path = state.config_dir.join("providers.toml");
    let existing = match tokio::fs::read_to_string(&providers_path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            tracing::error!(error = %e, path = %providers_path.display(), "failed to read providers.toml for patching");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to read providers.toml: {e}")),
                }),
            ));
        }
    };

    let patched =
        crate::config::patch::apply_patch(&existing, diff_map, "providers.toml").map_err(|msg| {
            tracing::warn!(error = %msg, path = %providers_path.display(), "providers.toml patch rejected");
            bad_request(msg)
        })?;

    Config::validate_providers_toml(&patched, &state.config_dir).map_err(|e| {
        tracing::warn!(error = %e, path = %providers_path.display(), "patched providers.toml failed validation");
        bad_request(e)
    })?;

    crate::util::fs::atomic_write(&providers_path, &patched)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, path = %providers_path.display(), "failed to write patched providers.toml");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write providers.toml: {e}")),
                }),
            )
        })?;

    // Trigger root reload — provider changes affect model resolution
    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Root).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

/// `POST /api/providers/validate` — validate providers TOML body without saving.
pub(super) async fn api_providers_validate(
    State(state): State<ConfigApiState>,
    body: String,
) -> Json<ValidateResponse> {
    match Config::validate_providers_toml(&body, &state.config_dir) {
        Ok(()) => Json(ValidateResponse {
            valid: true,
            error: None,
        }),
        Err(e) => Json(ValidateResponse {
            valid: false,
            error: Some(e),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fireworks_listing_drops_embedding_and_toolless_models() {
        let json = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "accounts/fireworks/models/glm-5p3", "kind": "HF_BASE_MODEL",
                 "supports_chat": true, "supports_tools": true},
                {"id": "accounts/fireworks/routers/glm-5p3-fast", "kind": "HF_BASE_MODEL",
                 "supports_chat": true, "supports_tools": true},
                {"id": "accounts/fireworks/models/qwen3-embedding-8b", "kind": "EMBEDDING_MODEL",
                 "supports_chat": true, "supports_tools": false},
                {"id": "accounts/fireworks/models/qwen3-reranker-8b", "kind": "EMBEDDING_MODEL",
                 "supports_chat": true, "supports_tools": false},
                {"id": "accounts/fireworks/models/base-only", "kind": "HF_BASE_MODEL",
                 "supports_chat": true, "supports_tools": false}
            ]
        });
        let ids: Vec<String> = parse_fireworks_chat_models(&json)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "accounts/fireworks/models/glm-5p3".to_string(),
                "accounts/fireworks/routers/glm-5p3-fast".to_string(),
            ],
            "only tool-capable chat models can run agent turns"
        );
    }

    #[test]
    fn fireworks_listing_rejects_unexpected_shape() {
        let err = parse_fireworks_chat_models(&serde_json::json!({"models": []})).err();
        assert_eq!(err.as_deref(), Some("missing data array"));
    }

    #[test]
    fn openai_listing_includes_fine_tunes() {
        let json = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "gpt-5", "object": "model"},
                {"id": "ft:gpt-5-mini:acme::abc123", "object": "model"},
                {"id": "text-embedding-3-small", "object": "model"}
            ]
        });
        let ids: Vec<String> = parse_openai_models(&json)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "gpt-5".to_string(),
                "ft:gpt-5-mini:acme::abc123".to_string(),
                "text-embedding-3-small".to_string(),
            ],
            "every listed model is offered, fine-tunes included"
        );
    }
}
