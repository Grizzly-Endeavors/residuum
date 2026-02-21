//! Reflector: compresses the observation log when it exceeds a token threshold.
//!
//! Sends the full observation log to an LLM which reorganizes and merges
//! observations while preserving chronology and context tags.

use chrono::Utc;

use crate::config::DEFAULT_REFLECTOR_THRESHOLD;
use crate::error::IronclawError;
use crate::memory::log_store::{load_observation_log, save_observation_log};
use crate::memory::tokens::estimate_tokens;
use crate::memory::types::{Observation, ObservationLog, Visibility};
use crate::models::{CompletionOptions, Message, ModelProvider};
use crate::workspace::layout::WorkspaceLayout;

/// Reflector configuration.
#[derive(Debug, Clone)]
pub struct ReflectorConfig {
    /// Minimum estimated tokens in the observation log before reflection triggers.
    pub threshold_tokens: usize,
}

impl Default for ReflectorConfig {
    fn default() -> Self {
        Self {
            threshold_tokens: DEFAULT_REFLECTOR_THRESHOLD,
        }
    }
}

/// The reflector compresses observation logs via LLM-driven reorganization.
pub struct Reflector {
    provider: Box<dyn ModelProvider>,
    config: ReflectorConfig,
}

impl Reflector {
    /// Create a new reflector with the given provider and config.
    #[must_use]
    pub fn new(provider: Box<dyn ModelProvider>, config: ReflectorConfig) -> Self {
        Self { provider, config }
    }

    /// Check whether the observation log exceeds the reflection threshold.
    #[must_use]
    pub fn should_reflect(&self, log: &ObservationLog) -> bool {
        let serialized = match serde_json::to_string(log) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize observation log for threshold check");
                return false;
            }
        };
        estimate_tokens(&serialized) >= self.config.threshold_tokens
    }

    /// Reflect on the observation log: compress and replace it.
    ///
    /// Sends the full log to the model for reorganization, then replaces
    /// the observation log file with the compressed version.
    ///
    /// # Errors
    /// Returns an error if the LLM call fails or file persistence fails.
    pub async fn reflect(&self, layout: &WorkspaceLayout) -> Result<ObservationLog, IronclawError> {
        let log = load_observation_log(&layout.observations_json()).await?;

        if log.is_empty() {
            return Ok(log);
        }

        // Load system prompt from disk, falling back to embedded constant.
        let system_prompt = tokio::fs::read_to_string(layout.reflector_md())
            .await
            .ok()
            .and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
            .unwrap_or_else(|| REFLECTION_SYSTEM_PROMPT.to_string());

        // Serialize the flat observations for the LLM prompt — keep full objects
        // so the model has project_context info for intelligent merging.
        let serialized = serde_json::to_string_pretty(&log.observations)
            .map_err(|e| IronclawError::Memory(format!("failed to serialize observations: {e}")))?;

        let messages = build_reflection_prompt(&serialized, &system_prompt);

        // Call the model
        let response = self
            .provider
            .complete(&messages, &[], &CompletionOptions::default())
            .await
            .map_err(IronclawError::Model)?;

        // Parse the bare string-array response into a compressed log.
        let compressed = parse_reflection_response(&response.content)?;

        if compressed.is_empty() {
            return Err(IronclawError::Memory(
                "reflector returned empty observations, refusing to replace observation log".into(),
            ));
        }

        // Backup observation log before replacement
        let obs_path = layout.observations_json();
        let backup_path = obs_path.with_extension("json.bak");
        if let Err(e) = tokio::fs::copy(&obs_path, &backup_path).await {
            tracing::warn!(error = %e, "failed to create observation log backup before reflection");
        }

        // Replace the observation log
        save_observation_log(&obs_path, &compressed).await?;

        tracing::info!(
            original_observations = log.len(),
            compressed_observations = compressed.len(),
            "reflection complete"
        );

        Ok(compressed)
    }
}

/// Build the reflection prompt with the serialized observation list.
fn build_reflection_prompt(serialized_observations: &str, system_prompt: &str) -> Vec<Message> {
    vec![
        Message::system(system_prompt),
        Message::user(format!(
            "Reorganize and compress these observations:\n\n{serialized_observations}"
        )),
    ]
}

/// Embedded fallback system prompt for the reflector.
///
/// Used when `memory/REFLECTOR.md` is absent. The workspace bootstrap writes
/// this same content to disk so users can customise it without recompiling.
const REFLECTION_SYSTEM_PROMPT: &str = r#"You are a memory reorganization system. Given a list of observations, merge and deduplicate them to reduce size while preserving all important information.

Return ONLY a JSON array of merged observation strings. Example:
["merged fact one", "merged fact two"]

Rules:
- Merge related observations into single, precise sentences
- Do NOT summarize — preserve specific details
- Remove redundant or duplicate observations
- Each output string should be a complete, self-contained sentence

Return ONLY a valid JSON array of strings, no markdown fencing, no explanation."#;

/// Parse the model's reflection response into an `ObservationLog`.
///
/// Expects a bare JSON array of strings: `["obs 1", "obs 2", ...]`
/// Each string becomes an [`Observation`] with `project_context = "general"`.
///
/// # Errors
/// Returns an error if the response cannot be parsed.
fn parse_reflection_response(content: &str) -> Result<ObservationLog, IronclawError> {
    let trimmed = content.trim();
    let json_str = crate::memory::strip_code_fences(trimmed);

    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse reflector response as JSON: {e}\nresponse: {trimmed}"
        ))
    })?;

    let strings = value.as_array().ok_or_else(|| {
        IronclawError::Memory(format!(
            "reflector response is not a JSON array\nresponse: {trimmed}"
        ))
    })?;

    let now = Utc::now();
    let mut log = ObservationLog::new();

    for item in strings {
        if let Some(obs_content) = item.as_str() {
            log.push(Observation {
                timestamp: now,
                project_context: "general".to_string(),
                source_episodes: vec![],
                visibility: Visibility::User,
                content: obs_content.to_string(),
            });
        }
    }

    Ok(log)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::types::Visibility;
    use crate::models::{ModelError, ModelResponse, ToolDefinition};
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// See `MockProvider` in `agent::tests` for duplication rationale.
    struct MockReflectorProvider {
        response: String,
        call_count: Arc<AtomicUsize>,
    }

    impl MockReflectorProvider {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockReflectorProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ModelResponse::new(self.response.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-reflector"
        }
    }

    fn sample_observation(episode_id: &str, ctx: &str) -> Observation {
        Observation {
            timestamp: Utc::now(),
            project_context: ctx.to_string(),
            source_episodes: vec![episode_id.to_string()],
            visibility: Visibility::User,
            content: format!("observation from {episode_id}"),
        }
    }

    const COMPRESSED_RESPONSE: &str =
        r#"["workspace uses flat layout", "identity files loaded at startup"]"#;

    #[test]
    fn should_reflect_below_threshold() {
        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 100_000,
            },
        );

        let mut log = ObservationLog::new();
        log.push(sample_observation("ep-001", "test"));

        assert!(
            !reflector.should_reflect(&log),
            "small log should not trigger reflection"
        );
    }

    #[test]
    fn should_reflect_above_threshold() {
        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 10,
            },
        );

        let mut log = ObservationLog::new();
        log.push(sample_observation("ep-001", "test"));

        assert!(
            reflector.should_reflect(&log),
            "log exceeding threshold should trigger"
        );
    }

    #[test]
    fn parse_reflection_converts_to_flat_observations() {
        let log = parse_reflection_response(COMPRESSED_RESPONSE).unwrap();

        // COMPRESSED_RESPONSE has 2 observation strings → 2 Observations
        assert_eq!(log.len(), 2, "should have two observations");
        assert_eq!(
            log.observations.first().map(|o| o.content.as_str()),
            Some("workspace uses flat layout"),
            "first observation content should match"
        );
        assert_eq!(
            log.observations.first().map(|o| o.project_context.as_str()),
            Some("general"),
            "project_context defaults to general"
        );
        // Reflector observations have empty source_episodes
        assert!(
            log.observations
                .first()
                .is_some_and(|o| o.source_episodes.is_empty()),
            "reflector observations should have empty source_episodes"
        );
    }

    #[test]
    fn parse_reflection_handles_code_fences() {
        let fenced = format!("```json\n{COMPRESSED_RESPONSE}\n```");
        let log = parse_reflection_response(&fenced).unwrap();
        assert_eq!(log.len(), 2, "should parse despite code fences");
    }

    #[tokio::test]
    async fn reflect_replaces_observation_log() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        // Write initial log with 2 observations
        let mut initial_log = ObservationLog::new();
        initial_log.push(sample_observation("ep-001", "ironclaw/workspace"));
        initial_log.push(sample_observation("ep-002", "ironclaw/workspace"));
        save_observation_log(&layout.observations_json(), &initial_log)
            .await
            .unwrap();

        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 10,
            },
        );

        // COMPRESSED_RESPONSE yields 2 observations
        let result = reflector.reflect(&layout).await.unwrap();
        assert_eq!(
            result.len(),
            2,
            "compressed log should have two observations"
        );

        // Verify file was replaced
        let reloaded = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(reloaded.len(), 2, "file should contain compressed log");
    }

    #[tokio::test]
    async fn reflect_empty_log_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig::default(),
        );

        let result = reflector.reflect(&layout).await.unwrap();
        assert!(result.is_empty(), "empty log should return empty");
    }

    #[test]
    fn parse_reflection_empty_array_yields_empty_log() {
        let log = parse_reflection_response("[]").unwrap();
        // Empty array → empty log (error check is in reflect())
        assert!(log.is_empty(), "empty array should yield empty log");
    }

    #[test]
    fn parse_reflection_non_array_errors() {
        let result = parse_reflection_response(r#"{"episodes": []}"#);
        assert!(result.is_err(), "non-array response should error");
    }

    #[tokio::test]
    async fn reflect_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let mut initial_log = ObservationLog::new();
        initial_log.push(sample_observation("ep-001", "ironclaw/workspace"));
        initial_log.push(sample_observation("ep-002", "ironclaw/workspace"));
        save_observation_log(&layout.observations_json(), &initial_log)
            .await
            .unwrap();

        let original_content = tokio::fs::read_to_string(&layout.observations_json())
            .await
            .unwrap();

        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 10,
            },
        );

        reflector.reflect(&layout).await.unwrap();

        let backup_path = layout.observations_json().with_extension("json.bak");
        assert!(backup_path.exists(), "backup file should exist");

        let backup_content = tokio::fs::read_to_string(&backup_path).await.unwrap();
        assert_eq!(
            backup_content, original_content,
            "backup should contain original log"
        );
    }

    #[tokio::test]
    async fn reflect_rejects_empty_llm_response() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let mut initial_log = ObservationLog::new();
        initial_log.push(sample_observation("ep-001", "test"));
        save_observation_log(&layout.observations_json(), &initial_log)
            .await
            .unwrap();

        let empty_array = "[]";
        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(empty_array)),
            ReflectorConfig {
                threshold_tokens: 10,
            },
        );

        let result = reflector.reflect(&layout).await;
        assert!(result.is_err(), "empty array response should error");

        // Original log should be preserved
        let preserved = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            preserved.len(),
            1,
            "original observation log should be preserved"
        );
    }
}
