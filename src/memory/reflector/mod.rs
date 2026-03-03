//! Reflector: compresses the observation log when it exceeds a token threshold.
//!
//! Sends the full observation log to an LLM which reorganizes and merges
//! observations while preserving chronology and context tags.

mod parse;
mod prompt;

use chrono_tz::Tz;

use crate::config::DEFAULT_REFLECTOR_THRESHOLD;
use crate::error::ResiduumError;
use crate::memory::log_store::{load_observation_log, save_observation_log};
use crate::memory::tokens::estimate_tokens;
use crate::memory::types::ObservationLog;
use crate::models::{CompletionOptions, ModelProvider, ResponseFormat};
use crate::workspace::layout::WorkspaceLayout;
use parse::parse_reflection_response;
use prompt::{REFLECTION_CONTENT_PROMPT, build_reflection_prompt, reflector_response_schema};

/// Reflector configuration.
#[derive(Debug, Clone)]
pub struct ReflectorConfig {
    /// Minimum estimated tokens in the observation log before reflection triggers.
    pub threshold_tokens: usize,
    /// Timezone used for timestamps in observations.
    pub tz: Tz,
}

impl Default for ReflectorConfig {
    fn default() -> Self {
        Self {
            threshold_tokens: DEFAULT_REFLECTOR_THRESHOLD,
            tz: chrono_tz::UTC,
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

    /// Create a disabled reflector that never triggers.
    ///
    /// Uses a `NullProvider` and `usize::MAX` threshold so reflection
    /// never fires. Used when memory subsystem initialization fails.
    #[must_use]
    pub fn disabled(tz: Tz) -> Self {
        Self {
            provider: Box::new(crate::models::null::NullProvider),
            config: ReflectorConfig {
                threshold_tokens: usize::MAX,
                tz,
            },
        }
    }

    /// Replace the reflector's configuration (e.g. after a config reload).
    pub fn update_config(&mut self, config: ReflectorConfig) {
        tracing::info!(
            old_threshold = self.config.threshold_tokens,
            new_threshold = config.threshold_tokens,
            "updating reflector config"
        );
        self.config = config;
    }

    /// Replace the model provider (e.g. after a provider config change).
    pub fn swap_provider(&mut self, provider: Box<dyn ModelProvider>) {
        tracing::info!("swapping reflector model provider");
        self.provider = provider;
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
    pub async fn reflect(&self, layout: &WorkspaceLayout) -> Result<ObservationLog, ResiduumError> {
        let log = load_observation_log(&layout.observations_json()).await?;

        if log.is_empty() {
            return Ok(log);
        }

        // Load content guidance from disk, falling back to embedded constant.
        let content_guidance = tokio::fs::read_to_string(layout.reflector_md())
            .await
            .ok()
            .and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
            .unwrap_or_else(|| REFLECTION_CONTENT_PROMPT.to_string());

        // Serialize the flat observations for the LLM prompt — keep full objects
        // so the model has project_context and timestamp info for intelligent merging.
        let serialized = serde_json::to_string_pretty(&log.observations)
            .map_err(|e| ResiduumError::Memory(format!("failed to serialize observations: {e}")))?;

        let messages = build_reflection_prompt(&serialized, &content_guidance);

        // Call the model with structured output
        let options = CompletionOptions {
            response_format: ResponseFormat::JsonSchema {
                name: "reflector_compression".to_string(),
                schema: reflector_response_schema(),
            },
            ..CompletionOptions::default()
        };
        let response = self
            .provider
            .complete(&messages, &[], &options)
            .await
            .map_err(ResiduumError::Model)?;

        // Parse the object-array response into a compressed log.
        let compressed = parse_reflection_response(&response.content, self.config.tz)?;

        if compressed.is_empty() {
            return Err(ResiduumError::Memory(
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

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::log_store::save_observation_log;
    use crate::memory::test_helpers::MockMemoryProvider;
    use crate::memory::types::{Observation, Visibility};
    use parse::parse_reflection_response;

    fn sample_observation(episode_id: &str, ctx: &str) -> Observation {
        Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: ctx.to_string(),
            source_episodes: vec![episode_id.to_string()],
            visibility: Visibility::User,
            content: format!("observation from {episode_id}"),
        }
    }

    const COMPRESSED_RESPONSE: &str = r#"{
        "observations": [
            {"content": "workspace uses flat layout", "timestamp": "2026-02-21T14:30", "project_context": "residuum/workspace", "visibility": "user"},
            {"content": "identity files loaded at startup", "timestamp": "2026-02-21T14:31", "project_context": "residuum/workspace", "visibility": "user"}
        ]
    }"#;

    #[test]
    fn should_reflect_below_threshold() {
        let reflector = Reflector::new(
            Box::new(MockMemoryProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 100_000,
                tz: chrono_tz::UTC,
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
            Box::new(MockMemoryProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 10,
                tz: chrono_tz::UTC,
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
        let log = parse_reflection_response(COMPRESSED_RESPONSE, chrono_tz::UTC).unwrap();

        // COMPRESSED_RESPONSE has 2 observation objects → 2 Observations
        assert_eq!(log.len(), 2, "should have two observations");
        assert_eq!(
            log.observations.first().map(|o| o.content.as_str()),
            Some("workspace uses flat layout"),
            "first observation content should match"
        );
        assert_eq!(
            log.observations.first().map(|o| o.project_context.as_str()),
            Some("residuum/workspace"),
            "project_context should be preserved from JSON"
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
        let log = parse_reflection_response(&fenced, chrono_tz::UTC).unwrap();
        assert_eq!(log.len(), 2, "should parse despite code fences");
    }

    #[test]
    fn parse_reflection_preserves_project_context() {
        let response = r#"[
            {"content": "obs from residuum", "timestamp": "2026-02-21T14:30", "project_context": "residuum/memory", "visibility": "user"},
            {"content": "obs from devops", "timestamp": "2026-02-21T14:31", "project_context": "devops/k8s", "visibility": "user"}
        ]"#;
        let log = parse_reflection_response(response, chrono_tz::UTC).unwrap();

        assert_eq!(log.len(), 2, "should have two observations");
        assert_eq!(
            log.observations.first().map(|o| o.project_context.as_str()),
            Some("residuum/memory"),
            "first project_context should round-trip"
        );
        assert_eq!(
            log.observations.get(1).map(|o| o.project_context.as_str()),
            Some("devops/k8s"),
            "second project_context should round-trip"
        );
    }

    #[test]
    fn parse_reflection_preserves_visibility() {
        let response = r#"[
            {"content": "background obs", "timestamp": "2026-02-21T03:00", "project_context": "pulse", "visibility": "background"},
            {"content": "user obs", "timestamp": "2026-02-21T14:30", "project_context": "general", "visibility": "user"}
        ]"#;
        let log = parse_reflection_response(response, chrono_tz::UTC).unwrap();

        assert_eq!(log.len(), 2, "should have two observations");
        assert_eq!(
            log.observations.first().map(|o| &o.visibility),
            Some(&Visibility::Background),
            "background visibility should round-trip"
        );
        assert_eq!(
            log.observations.get(1).map(|o| &o.visibility),
            Some(&Visibility::User),
            "user visibility should round-trip"
        );
    }

    #[test]
    fn parse_reflection_preserves_timestamp() {
        let response = r#"[
            {"content": "an observation", "timestamp": "2026-02-21T14:30", "project_context": "test", "visibility": "user"}
        ]"#;
        let log = parse_reflection_response(response, chrono_tz::UTC).unwrap();
        let ts = log.observations.first().map(|o| o.timestamp).unwrap();
        assert_eq!(ts.format("%Y-%m-%dT%H:%M").to_string(), "2026-02-21T14:30");
    }

    #[test]
    fn parse_reflection_missing_timestamp_falls_back() {
        let response = r#"[
            {"content": "an observation", "project_context": "test", "visibility": "user"}
        ]"#;
        let log = parse_reflection_response(response, chrono_tz::UTC).unwrap();
        // Should succeed with now_local() fallback — just verify observation was parsed
        assert_eq!(
            log.len(),
            1,
            "should parse observation despite missing timestamp"
        );
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
        initial_log.push(sample_observation("ep-001", "residuum/workspace"));
        initial_log.push(sample_observation("ep-002", "residuum/workspace"));
        save_observation_log(&layout.observations_json(), &initial_log)
            .await
            .unwrap();

        let reflector = Reflector::new(
            Box::new(MockMemoryProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 10,
                tz: chrono_tz::UTC,
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
            Box::new(MockMemoryProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig::default(),
        );

        let result = reflector.reflect(&layout).await.unwrap();
        assert!(result.is_empty(), "empty log should return empty");
    }

    #[test]
    fn parse_reflection_empty_array_yields_empty_log() {
        let log = parse_reflection_response("[]", chrono_tz::UTC).unwrap();
        // Empty array → empty log (error check is in reflect())
        assert!(log.is_empty(), "empty array should yield empty log");
    }

    #[test]
    fn parse_reflection_malformed_object_errors() {
        // Object without "observations" key — fails typed path, fails array check
        let result = parse_reflection_response(r#"{"episodes": []}"#, chrono_tz::UTC);
        assert!(
            result.is_err(),
            "object without observations field should error"
        );
    }

    #[test]
    fn parse_reflection_bare_array_fallback() {
        let bare_array = r#"[
            {"content": "obs from bare array", "timestamp": "2026-02-21T14:30", "project_context": "test", "visibility": "user"}
        ]"#;
        let log = parse_reflection_response(bare_array, chrono_tz::UTC).unwrap();
        assert_eq!(log.len(), 1, "bare array fallback should parse");
        assert_eq!(
            log.observations.first().map(|o| o.content.as_str()),
            Some("obs from bare array"),
            "content should match"
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
        initial_log.push(sample_observation("ep-001", "residuum/workspace"));
        initial_log.push(sample_observation("ep-002", "residuum/workspace"));
        save_observation_log(&layout.observations_json(), &initial_log)
            .await
            .unwrap();

        let original_content = tokio::fs::read_to_string(&layout.observations_json())
            .await
            .unwrap();

        let reflector = Reflector::new(
            Box::new(MockMemoryProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 10,
                tz: chrono_tz::UTC,
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
            Box::new(MockMemoryProvider::new(empty_array)),
            ReflectorConfig {
                threshold_tokens: 10,
                tz: chrono_tz::UTC,
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

    #[test]
    fn update_config_changes_threshold() {
        let mut reflector = Reflector::new(
            Box::new(MockMemoryProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig {
                threshold_tokens: 1000,
                tz: chrono_tz::UTC,
            },
        );

        // Small log should NOT trigger at threshold=1000
        let mut log = ObservationLog::new();
        log.push(sample_observation("ep-001", "test"));
        assert!(!reflector.should_reflect(&log));

        // Lower the threshold
        reflector.update_config(ReflectorConfig {
            threshold_tokens: 10,
            tz: chrono_tz::US::Eastern,
        });

        // Same log should now trigger at threshold=10
        assert!(reflector.should_reflect(&log));
    }

    #[test]
    fn swap_provider_changes_model() {
        let mut reflector = Reflector::new(
            Box::new(MockMemoryProvider::new(COMPRESSED_RESPONSE)),
            ReflectorConfig::default(),
        );

        let new_response = r#"{
            "observations": [
                {"content": "from new provider", "timestamp": "2026-02-21T14:30", "project_context": "test", "visibility": "user"}
            ]
        }"#;
        reflector.swap_provider(Box::new(MockMemoryProvider::new(new_response)));

        // Verify the provider was swapped without panic
        let log = ObservationLog::new();
        assert!(!reflector.should_reflect(&log));
    }
}
