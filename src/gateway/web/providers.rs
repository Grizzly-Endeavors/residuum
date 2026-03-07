//! Provider model listing endpoints and types.

use std::time::Duration;

use axum::extract::State;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::config::secrets::SecretStore;

use super::ConfigApiState;

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

	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(10))
		.build()
		.unwrap_or_default();

	let result = match req.provider.as_str() {
		"anthropic" => {
			fetch_anthropic_models(&client, resolved_key.as_deref(), req.url.as_deref()).await
		}
		"openai" => fetch_openai_models(&client, resolved_key.as_deref(), req.url.as_deref()).await,
		"gemini" => fetch_gemini_models(&client, resolved_key.as_deref(), req.url.as_deref()).await,
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
	// NOTE: this logic is duplicated in models::anthropic::AnthropicClient::complete
	if key.starts_with("sk-ant-oat01-") {
		req_builder = req_builder
			.header("Authorization", format!("Bearer {key}"))
			.header("anthropic-beta", "oauth-2025-04-20");
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
	let data = json
		.get("data")
		.and_then(|v| v.as_array())
		.ok_or("missing data array")?;

	let skip_prefixes = [
		"ft:",
		"dall-e",
		"tts-",
		"whisper",
		"text-embedding",
		"babbage",
		"davinci",
	];

	Ok(data
		.iter()
		.filter_map(|m| {
			let id = m.get("id")?.as_str()?;
			if skip_prefixes.iter().any(|prefix| id.starts_with(prefix)) {
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
