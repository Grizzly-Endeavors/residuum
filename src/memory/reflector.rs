//! Reflector: compresses the observation log when it exceeds a token threshold.
//!
//! Sends the full observation log to an LLM which reorganizes and merges
//! episodes while preserving chronology and context tags.

use crate::config::DEFAULT_REFLECTOR_THRESHOLD;
use crate::error::IronclawError;
use crate::memory::log_store::{load_observation_log, save_observation_log};
use crate::memory::tokens::estimate_tokens;
use crate::memory::types::ObservationLog;
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

        let source_ids: Vec<String> = log.episodes.iter().map(|ep| ep.id.clone()).collect();

        // Build reflection prompt
        let serialized = serde_json::to_string_pretty(&log).map_err(|e| {
            IronclawError::Memory(format!("failed to serialize observation log: {e}"))
        })?;

        let messages = build_reflection_prompt(&serialized);

        // Call the model
        let response = self
            .provider
            .complete(&messages, &[], &CompletionOptions::default())
            .await
            .map_err(IronclawError::Model)?;

        // Parse the compressed log
        let compressed = parse_reflection_response(&response.content, &source_ids)?;

        // Backup observation log before replacement
        let obs_path = layout.observations_json();
        let backup_path = obs_path.with_extension("json.bak");
        if let Err(e) = tokio::fs::copy(&obs_path, &backup_path).await {
            tracing::warn!(error = %e, "failed to create observation log backup before reflection");
        }

        // Replace the observation log
        save_observation_log(&obs_path, &compressed).await?;

        tracing::info!(
            original_episodes = log.len(),
            compressed_episodes = compressed.len(),
            "reflection complete"
        );

        Ok(compressed)
    }
}

/// Build the reflection prompt with the serialized observation log.
fn build_reflection_prompt(serialized_log: &str) -> Vec<Message> {
    vec![
        Message::system(REFLECTION_SYSTEM_PROMPT),
        Message::user(format!(
            "Reorganize and compress this observation log:\n\n{serialized_log}"
        )),
    ]
}

/// System prompt for the reflector model.
const REFLECTION_SYSTEM_PROMPT: &str = r#"You are a memory reorganization system. Given an observation log (JSON array of episodes), reorganize and merge related episodes to reduce size while preserving all important information.

Rules:
- Reorganize by topic/context, merging episodes with the same context
- Do NOT summarize — preserve the specific details and observations
- Preserve chronological order within each topic
- Preserve all context tags exactly as they appear
- Remove redundant or duplicate observations
- Each reflected episode should use "ref-NNN" as the id format
- Include a "source_episodes" field listing the original episode IDs that were merged

Return ONLY a valid JSON object with an "episodes" field containing the reorganized array. No markdown fencing, no explanation."#;

/// Parse the model's reflection response into an `ObservationLog`.
///
/// # Errors
/// Returns an error if the response cannot be parsed.
fn parse_reflection_response(
    content: &str,
    source_ids: &[String],
) -> Result<ObservationLog, IronclawError> {
    let trimmed = content.trim();
    let json_str = crate::memory::strip_code_fences(trimmed);

    // Try parsing as ObservationLog directly
    let mut log: ObservationLog = serde_json::from_str(json_str).map_err(|e| {
        IronclawError::Memory(format!(
            "failed to parse reflector response as JSON: {e}\nresponse: {trimmed}"
        ))
    })?;

    if log.is_empty() {
        return Err(IronclawError::Memory(
            "reflector returned empty episodes, refusing to replace observation log".into(),
        ));
    }

    // Ensure all reflected episodes have source_episodes if not set by the model
    for episode in &mut log.episodes {
        if episode.source_episodes.is_empty() {
            episode.source_episodes = source_ids.to_vec();
        }
    }

    Ok(log)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::types::Episode;
    use crate::models::{ModelError, ModelResponse, ToolDefinition};
    use async_trait::async_trait;
    use chrono::NaiveDate;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn sample_episode(id: &str, ctx: &str) -> Episode {
        Episode {
            id: id.to_string(),
            date: NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            start: "started".to_string(),
            end: "ended".to_string(),
            context: ctx.to_string(),
            observations: vec![format!("observation from {id}")],
            source_episodes: vec![],
        }
    }

    const COMPRESSED_RESPONSE: &str = r#"{
        "episodes": [
            {
                "id": "ref-001",
                "date": "2026-02-19",
                "start": "workspace exploration",
                "end": "file operations complete",
                "context": "ironclaw/workspace",
                "observations": [
                    "workspace uses flat layout",
                    "identity files loaded at startup"
                ],
                "source_episodes": ["ep-001", "ep-002"]
            }
        ]
    }"#;

    #[test]
    fn should_reflect_below_threshold() {
        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 100_000,
            },
        );

        let mut log = ObservationLog::new();
        log.push(sample_episode("ep-001", "test"));

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
        log.push(sample_episode("ep-001", "test"));

        assert!(
            reflector.should_reflect(&log),
            "log exceeding threshold should trigger"
        );
    }

    #[test]
    fn parse_reflection_preserves_source_episodes() {
        let log = parse_reflection_response(COMPRESSED_RESPONSE, &[]).unwrap();

        assert_eq!(log.len(), 1, "should have one reflected episode");
        let episode = log.episodes.first().unwrap();
        assert_eq!(episode.id, "ref-001", "should have ref- prefix");
        assert_eq!(
            episode.source_episodes,
            vec!["ep-001", "ep-002"],
            "should preserve source episodes from model"
        );
    }

    #[test]
    fn parse_reflection_fills_source_ids_when_missing() {
        let response = r#"{
            "episodes": [{
                "id": "ref-001",
                "date": "2026-02-19",
                "start": "start",
                "end": "end",
                "context": "test",
                "observations": ["obs"]
            }]
        }"#;

        let source_ids = vec!["ep-001".to_string(), "ep-002".to_string()];
        let log = parse_reflection_response(response, &source_ids).unwrap();

        let episode = log.episodes.first().unwrap();
        assert_eq!(
            episode.source_episodes, source_ids,
            "should fill source_episodes from original IDs"
        );
    }

    #[test]
    fn parse_reflection_handles_code_fences() {
        let fenced = format!("```json\n{COMPRESSED_RESPONSE}\n```");
        let log = parse_reflection_response(&fenced, &[]).unwrap();
        assert_eq!(log.len(), 1, "should parse despite code fences");
    }

    #[tokio::test]
    async fn reflect_replaces_observation_log() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        // Write initial log
        let mut initial_log = ObservationLog::new();
        initial_log.push(sample_episode("ep-001", "ironclaw/workspace"));
        initial_log.push(sample_episode("ep-002", "ironclaw/workspace"));
        save_observation_log(&layout.observations_json(), &initial_log)
            .await
            .unwrap();

        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 10,
            },
        );

        let result = reflector.reflect(&layout).await.unwrap();
        assert_eq!(result.len(), 1, "compressed log should have one episode");

        // Verify file was replaced
        let reloaded = load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(reloaded.len(), 1, "file should contain compressed log");
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
    fn parse_reflection_rejects_empty_episodes() {
        let empty_response = r#"{"episodes": []}"#;
        let source_ids = vec!["ep-001".to_string()];
        let result = parse_reflection_response(empty_response, &source_ids);
        assert!(result.is_err(), "empty episodes should be rejected");
        assert!(
            result.unwrap_err().to_string().contains("empty episodes"),
            "error should mention empty episodes"
        );
    }

    #[tokio::test]
    async fn reflect_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let mut initial_log = ObservationLog::new();
        initial_log.push(sample_episode("ep-001", "ironclaw/workspace"));
        initial_log.push(sample_episode("ep-002", "ironclaw/workspace"));
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
        initial_log.push(sample_episode("ep-001", "test"));
        save_observation_log(&layout.observations_json(), &initial_log)
            .await
            .unwrap();

        let empty_episodes = r#"{"episodes": []}"#;
        let reflector = Reflector::new(
            Box::new(MockReflectorProvider::new(empty_episodes)),
            ReflectorConfig {
                threshold_tokens: 10,
            },
        );

        let result = reflector.reflect(&layout).await;
        assert!(result.is_err(), "empty LLM response should error");

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
