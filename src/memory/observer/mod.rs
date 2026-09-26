//! Observer: extracts observations and a narrative from recent messages via LLM.
//!
//! Runs synchronously after the agent completes a turn when the accumulated
//! recent message token count exceeds the configured threshold. Extraction
//! has no side effects — see [`Observer::extract`]; persisting the result as
//! an episode is [`crate::memory::merge_writer::MemoryMergeWriter`]'s job.

mod parse;
mod prompt;

use std::sync::Arc;

use anyhow::Context;
use chrono::NaiveDateTime;
use chrono_tz::Tz;

use crate::config::{
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
};
use crate::inference::{CompletionOptions, InferenceProvider, ResponseFormat};
use crate::memory::recent_messages::RecentMessage;
use crate::memory::types::Visibility;
use crate::workspace::layout::WorkspaceLayout;
use parse::parse_observer_response;
use prompt::{EXTRACTION_CONTENT_PROMPT, build_extraction_prompt, observer_response_schema};

/// A single observation extracted from a conversation segment, before it is
/// assigned an episode id or written anywhere.
#[derive(Debug, Clone)]
pub struct ExtractedObservation {
    /// When the observed event happened, at minute precision.
    pub timestamp: NaiveDateTime,
    /// Whether the source turn was user-visible or a background turn.
    pub visibility: Visibility,
    /// The observation content as a single concise sentence.
    pub content: String,
}

/// Output of a pure extraction pass: observations and a narrative, plus the
/// plain messages that were extracted from. Carries no side effects — nothing
/// is written to disk, no episode id is allocated. Persisting an extraction
/// (episode id allocation, transcript/observation/index writes, embedding,
/// and the reflector check) is the memory merge writer's job.
pub struct Extraction {
    /// Narrative summary of the conversation at the time of extraction.
    pub narrative: Option<String>,
    /// The extracted observations.
    pub observations: Vec<ExtractedObservation>,
    /// The messages the extraction was run over (with their timestamp and
    /// visibility metadata intact), for the merge writer to persist as an
    /// episode transcript and interaction-pair chunks.
    pub messages: Vec<RecentMessage>,
}

/// What the observer thinks should happen after checking token thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserveAction {
    /// Token count is below the soft threshold — do nothing.
    None,
    /// Token count is at or above the soft threshold — start or reset the cooldown timer.
    StartCooldown,
    /// Token count is at or above the force threshold — observe immediately.
    ForceNow,
}

/// Observer configuration.
#[derive(Debug, Clone)]
pub struct ObserverConfig {
    /// Minimum estimated tokens in recent messages before observation triggers.
    pub threshold_tokens: usize,
    /// Cooldown period in seconds after the soft threshold is crossed.
    pub cooldown_secs: u64,
    /// Token threshold that forces immediate observation (bypasses cooldown).
    pub force_threshold_tokens: usize,
    /// Timezone used for timestamps in observations.
    pub tz: Tz,
    /// Per-role overrides for temperature and thinking.
    pub role_overrides: Option<crate::config::RoleOverrides>,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            threshold_tokens: DEFAULT_OBSERVER_THRESHOLD,
            cooldown_secs: DEFAULT_OBSERVER_COOLDOWN_SECS,
            force_threshold_tokens: DEFAULT_OBSERVER_FORCE_THRESHOLD,
            tz: chrono_tz::UTC,
            role_overrides: None,
        }
    }
}

/// Provider and config, guarded together so a config/provider reload never
/// observes a torn combination of the two.
struct ObserverInner {
    provider: Arc<dyn InferenceProvider>,
    config: ObserverConfig,
}

/// The observer extracts structured episodes from recent messages.
///
/// Provider and config live behind a lock, not owned directly, so `Observer`
/// can be shared (`Arc<Observer>`) with the background post-turn worker —
/// see `crate::gateway::post_turn` — while a config reload on the main loop
/// can still swap them in place. `extract` only ever holds the lock briefly,
/// to clone out an `Arc` and a couple of `Copy`/cheap-`Clone` fields, never
/// across the LLM call itself.
pub struct Observer {
    inner: std::sync::RwLock<ObserverInner>,
    /// Backs off the automatic extract trigger (threshold crossings) after a
    /// failure, instead of re-attempting — and re-spending, since this is an
    /// LLM call — on every later threshold crossing while recent messages
    /// keep accumulating unobserved. Manual observes (`observe --force`)
    /// bypass this deliberately and always attempt.
    automatic_failure: crate::util::BackoffTracker,
}

impl Observer {
    /// Create a new observer with the given provider and config.
    #[must_use]
    pub fn new(provider: Box<dyn InferenceProvider>, config: ObserverConfig) -> Self {
        Self {
            inner: std::sync::RwLock::new(ObserverInner {
                provider: Arc::from(provider),
                config,
            }),
            automatic_failure: crate::util::BackoffTracker::new(),
        }
    }

    /// Create a disabled observer that never triggers.
    ///
    /// Uses a `NullProvider` and `usize::MAX` thresholds so observation
    /// never fires. Used when memory subsystem initialization fails.
    #[must_use]
    pub fn disabled(tz: Tz) -> Self {
        Self {
            inner: std::sync::RwLock::new(ObserverInner {
                provider: Arc::new(crate::inference::providers::null::NullProvider),
                config: ObserverConfig {
                    threshold_tokens: usize::MAX,
                    cooldown_secs: u64::MAX,
                    force_threshold_tokens: usize::MAX,
                    tz,
                    role_overrides: None,
                },
            }),
            automatic_failure: crate::util::BackoffTracker::new(),
        }
    }

    fn read_inner(&self) -> std::sync::RwLockReadGuard<'_, ObserverInner> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_inner(&self) -> std::sync::RwLockWriteGuard<'_, ObserverInner> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The configured cooldown period in seconds.
    #[must_use]
    pub fn cooldown_secs(&self) -> u64 {
        self.read_inner().config.cooldown_secs
    }

    /// The soft observation threshold in tokens.
    #[must_use]
    pub fn threshold_tokens(&self) -> usize {
        self.read_inner().config.threshold_tokens
    }

    /// The force observation threshold in tokens (bypasses cooldown).
    #[must_use]
    pub fn force_threshold_tokens(&self) -> usize {
        self.read_inner().config.force_threshold_tokens
    }

    /// The configured timezone.
    #[must_use]
    pub fn timezone(&self) -> Tz {
        self.read_inner().config.tz
    }

    /// Tracks consecutive failures of the *automatic* extract trigger
    /// (threshold crossings), for backing off retries and telling the user
    /// once when a failure streak starts or clears. A manually forced
    /// observe doesn't consult this — it always attempts.
    #[must_use]
    pub fn automatic_failure_tracker(&self) -> &crate::util::BackoffTracker {
        &self.automatic_failure
    }

    /// Replace the observer's configuration (e.g. after a config reload).
    pub fn update_config(&self, config: ObserverConfig) {
        let mut inner = self.write_inner();
        tracing::debug!(
            old_threshold = inner.config.threshold_tokens,
            new_threshold = config.threshold_tokens,
            old_cooldown_secs = inner.config.cooldown_secs,
            new_cooldown_secs = config.cooldown_secs,
            old_force_threshold = inner.config.force_threshold_tokens,
            new_force_threshold = config.force_threshold_tokens,
            "updating observer config"
        );
        inner.config = config;
    }

    /// Replace the model provider (e.g. after a provider config change).
    pub fn swap_provider(&self, provider: Box<dyn InferenceProvider>) {
        tracing::debug!("swapping observer model provider");
        self.write_inner().provider = Arc::from(provider);
    }

    /// Check token thresholds and return the appropriate action.
    ///
    /// Returns `ForceNow` if tokens >= force threshold, `StartCooldown` if
    /// tokens >= soft threshold, or `None` if below both.
    #[must_use]
    pub fn check_thresholds(&self, recent_messages: &[RecentMessage]) -> ObserveAction {
        let tokens = estimate_recent_tokens(recent_messages);
        let inner = self.read_inner();
        if tokens >= inner.config.force_threshold_tokens {
            ObserveAction::ForceNow
        } else if tokens >= inner.config.threshold_tokens {
            ObserveAction::StartCooldown
        } else {
            ObserveAction::None
        }
    }

    /// Extract observations and a narrative from recent messages.
    ///
    /// Pure extraction: no episode id is allocated and nothing is written to
    /// disk. Persisting the result — allocating an episode id, writing the
    /// transcript and observation archives, indexing, embedding, and
    /// checking the reflector — is the memory merge writer's job, so that
    /// all persistence (main agent and session runs alike) is serialized
    /// through one writer.
    ///
    /// # Errors
    /// Returns an error if the LLM call fails or its response cannot be parsed.
    #[tracing::instrument(skip_all, fields(operation = "extract", message_count = recent_messages.len()))]
    pub async fn extract(
        &self,
        recent_messages: &[RecentMessage],
        layout: &WorkspaceLayout,
    ) -> anyhow::Result<Extraction> {
        if recent_messages.is_empty() {
            anyhow::bail!("no recent messages to extract from");
        }

        // Load content guidance from disk, falling back to embedded constant.
        let content_guidance = match tokio::fs::read_to_string(layout.observer_md()).await {
            Ok(s) if !s.trim().is_empty() => s,
            Ok(_) => EXTRACTION_CONTENT_PROMPT.to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                EXTRACTION_CONTENT_PROMPT.to_string()
            }
            Err(e) => {
                tracing::warn!(path = %layout.observer_md().display(), error = %e, "failed to read observer guidance, using default");
                EXTRACTION_CONTENT_PROMPT.to_string()
            }
        };

        // Build extraction prompt using full RecentMessage metadata (timestamps,
        // tool calls) so the observer LLM has complete context.
        let extraction_messages = build_extraction_prompt(recent_messages, &content_guidance);

        // Snapshot the provider (a cheap `Arc` clone) and the config fields
        // this call needs, then drop the lock before the LLM call below —
        // held only briefly, never across the `.await`, so a concurrent
        // config reload is never blocked on an in-flight extraction.
        let (provider, role_overrides, tz) = {
            let inner = self.read_inner();
            (
                Arc::clone(&inner.provider),
                inner.config.role_overrides.clone(),
                inner.config.tz,
            )
        };

        // Call the model with structured output, applying per-role overrides
        let ov = role_overrides.as_ref();
        let options = CompletionOptions {
            temperature: ov.and_then(|o| o.temperature),
            thinking: ov.and_then(|o| o.thinking.clone()),
            response_format: ResponseFormat::JsonSchema {
                name: "observer_extraction".to_string(),
                schema: observer_response_schema(),
            },
            ..CompletionOptions::default()
        };
        let response = provider
            .complete(&extraction_messages, &[], &options)
            .await
            .context("observer LLM call failed")?;

        // Parse the response into extraction results and optional narrative.
        let parsed = parse_observer_response(&response, tz)?;

        let messages: Vec<RecentMessage> = recent_messages.to_vec();
        let observations = parsed
            .extractions
            .into_iter()
            .map(|e| ExtractedObservation {
                timestamp: e.timestamp,
                visibility: e.visibility,
                content: e.content,
            })
            .collect();

        Ok(Extraction {
            narrative: parsed.narrative,
            observations,
            messages,
        })
    }
}

/// Estimate the total token count of recent messages.
fn estimate_recent_tokens(recent_messages: &[RecentMessage]) -> usize {
    recent_messages
        .iter()
        .map(|rm| crate::memory::tokens::estimate_single_message(&rm.message))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{InferenceResponse, Message, Role};
    use crate::memory::recent_messages::RecentMessage;
    use crate::memory::test_helpers::MockMemoryProvider;
    use crate::memory::types::Visibility;
    use parse::{parse_extraction_items, parse_observer_response};
    use prompt::{
        EXTRACTION_CONTENT_PROMPT, EXTRACTION_FORMAT_SPEC, build_extraction_prompt,
        format_recent_message,
    };

    const SAMPLE_RESPONSE: &str = r#"{
        "observations": [
            {"content": "workspace uses a flat directory layout", "timestamp": "2026-02-21T14:30", "visibility": "user"},
            {"content": "identity files are loaded at startup", "timestamp": "2026-02-21T14:31", "visibility": "user"}
        ],
        "narrative": ""
    }"#;

    fn make_recent_messages(count: usize) -> Vec<RecentMessage> {
        (0..count)
            .map(|i| RecentMessage {
                message: Message::user(format!(
                    "message {i} with enough content to contribute to token count - {}",
                    "a".repeat(100)
                )),
                timestamp: chrono::Utc::now().naive_utc(),
                visibility: Visibility::User,
            })
            .collect()
    }

    #[test]
    fn parse_observer_response_typed_object_format() {
        let response = InferenceResponse::new(SAMPLE_RESPONSE.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 2, "should have 2 extractions");
        assert_eq!(
            parsed.extractions.first().map(|e| e.content.as_str()),
            Some("workspace uses a flat directory layout"),
            "first extraction content should match"
        );
        assert_eq!(
            parsed.extractions.get(1).map(|e| e.content.as_str()),
            Some("identity files are loaded at startup"),
            "second extraction content should match"
        );
        assert!(parsed.narrative.is_none(), "empty narrative should be None");
    }

    #[test]
    fn parse_observer_response_legacy_array_format() {
        let bare_array = r#"[
            {"content": "workspace uses a flat directory layout", "timestamp": "2026-02-21T14:30", "visibility": "user"},
            {"content": "identity files are loaded at startup", "timestamp": "2026-02-21T14:31", "visibility": "user"}
        ]"#;
        let response = InferenceResponse::new(bare_array.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 2, "should have 2 extractions");
        assert!(
            parsed.narrative.is_none(),
            "legacy format should have no narrative"
        );
    }

    #[test]
    fn parse_observer_response_new_format() {
        let json = r#"{
            "observations": [
                {"content": "user prefers Rust", "timestamp": "2026-02-21T14:30", "visibility": "user"}
            ],
            "narrative": "We were discussing language preferences."
        }"#;
        let response = InferenceResponse::new(json.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 1, "should have 1 extraction");
        assert_eq!(
            parsed.narrative.as_deref(),
            Some("We were discussing language preferences."),
            "narrative should be extracted"
        );
    }

    #[test]
    fn parse_observer_response_narrative_missing() {
        let json = r#"{
            "observations": [
                {"content": "user prefers Rust", "timestamp": "2026-02-21T14:30", "visibility": "user"}
            ]
        }"#;
        let response = InferenceResponse::new(json.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 1, "should have 1 extraction");
        assert!(
            parsed.narrative.is_none(),
            "missing narrative should be None"
        );
    }

    #[test]
    fn parse_observer_response_with_code_fences() {
        let fenced = format!("```json\n{SAMPLE_RESPONSE}\n```");
        let response = InferenceResponse::new(fenced, vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 2, "should parse despite fences");
    }

    #[test]
    fn parse_observer_response_empty_narrative_is_none() {
        let json = r#"{
            "observations": [
                {"content": "user prefers Rust", "timestamp": "2026-02-21T14:30", "visibility": "user"}
            ],
            "narrative": ""
        }"#;
        let response = InferenceResponse::new(json.to_string(), vec![]);
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();

        assert_eq!(parsed.extractions.len(), 1, "should have 1 extraction");
        assert!(
            parsed.narrative.is_none(),
            "empty narrative string should be None"
        );
    }

    #[test]
    fn parse_observer_response_invalid_json_errors() {
        let response = InferenceResponse::new("not json at all".to_string(), vec![]);
        let result = parse_observer_response(&response, chrono_tz::UTC);
        assert!(result.is_err(), "invalid JSON should error");
    }

    #[test]
    fn parse_observer_response_empty_array_errors() {
        let response = InferenceResponse::new("[]".to_string(), vec![]);
        let result = parse_observer_response(&response, chrono_tz::UTC);
        assert!(result.is_err(), "empty array should error");
    }

    #[test]
    fn parse_observer_response_timestamp_minute_precision() {
        let response = InferenceResponse::new(
            r#"[{"content": "test obs", "timestamp": "2026-02-21T14:30", "visibility": "user"}]"#
                .to_string(),
            vec![],
        );
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();
        let ts = parsed.extractions.first().unwrap().timestamp;
        assert_eq!(ts.format("%Y-%m-%dT%H:%M").to_string(), "2026-02-21T14:30");
    }

    #[test]
    fn parse_observer_response_background_visibility() {
        let response = InferenceResponse::new(
            r#"[{"content": "cron job ran", "timestamp": "2026-02-21T03:00", "visibility": "background"}]"#
                .to_string(),
            vec![],
        );
        let parsed = parse_observer_response(&response, chrono_tz::UTC).unwrap();
        assert_eq!(
            parsed.extractions.first().map(|e| &e.visibility),
            Some(&Visibility::Background),
            "background visibility should be parsed"
        );
    }

    #[test]
    fn should_observe_below_threshold() {
        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 1000,
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(2);

        assert!(
            observer.check_thresholds(&messages) == ObserveAction::None,
            "should not observe below threshold"
        );
    }

    #[test]
    fn should_observe_above_threshold() {
        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
                force_threshold_tokens: 100_000,
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(5);

        assert_eq!(
            observer.check_thresholds(&messages),
            ObserveAction::StartCooldown,
            "should start cooldown above soft threshold but below force threshold"
        );
    }

    #[test]
    fn check_thresholds_below_soft() {
        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 100_000,
                force_threshold_tokens: 200_000,
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(2);
        assert_eq!(
            observer.check_thresholds(&messages),
            ObserveAction::None,
            "below soft threshold should return None"
        );
    }

    #[test]
    fn check_thresholds_between_soft_and_force() {
        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
                force_threshold_tokens: 100_000,
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(5);
        assert_eq!(
            observer.check_thresholds(&messages),
            ObserveAction::StartCooldown,
            "between soft and force should return StartCooldown"
        );
    }

    #[test]
    fn check_thresholds_above_force() {
        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
                force_threshold_tokens: 10,
                ..ObserverConfig::default()
            },
        );
        let messages = make_recent_messages(5);
        assert_eq!(
            observer.check_thresholds(&messages),
            ObserveAction::ForceNow,
            "above force threshold should return ForceNow"
        );
    }

    #[tokio::test]
    async fn extract_returns_observations_and_messages_without_persisting() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::create_dir_all(layout.episodes_dir())
            .await
            .unwrap();
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
                ..ObserverConfig::default()
            },
        );

        let messages = make_recent_messages(5);
        let result = observer.extract(&messages, &layout).await.unwrap();

        assert_eq!(
            result.observations.len(),
            2,
            "SAMPLE_RESPONSE has 2 observations"
        );
        assert_eq!(
            result.messages.len(),
            5,
            "extraction should carry the plain messages for the caller to persist"
        );
        assert!(
            !layout.episodes_dir().exists()
                || std::fs::read_dir(layout.episodes_dir())
                    .map_or(true, |mut d| d.next().is_none()),
            "extraction alone must not write any episode files — that's the merge writer's job"
        );
    }

    #[test]
    fn extraction_prompt_includes_messages() {
        let recent_messages = vec![RecentMessage {
            message: Message::user("test content"),
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::User,
        }];

        let prompt = build_extraction_prompt(&recent_messages, EXTRACTION_CONTENT_PROMPT);
        assert_eq!(prompt.len(), 2, "should have system + user message");
        assert_eq!(
            prompt.first().map(|m| m.role),
            Some(Role::System),
            "first should be system"
        );

        let system_content = prompt.first().map_or("", |m| m.content.as_str());
        assert!(
            system_content.contains(EXTRACTION_FORMAT_SPEC),
            "system prompt should always include format spec"
        );

        let user_content = prompt.get(1).map_or("", |m| m.content.as_str());
        assert!(
            user_content.contains("test content"),
            "should include message content"
        );
    }

    #[test]
    fn format_recent_message_includes_tool_calls() {
        use crate::inference::ToolCall;

        let rm = RecentMessage {
            message: Message::assistant(
                String::new(),
                Some(vec![ToolCall {
                    id: "call_abc".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                }]),
            ),
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::User,
        };

        let formatted = format_recent_message(&rm);
        assert!(formatted.contains("read_file"), "should include tool name");
        assert!(
            formatted.contains("call_abc"),
            "should include tool call id"
        );
        assert!(
            formatted.contains("src/main.rs"),
            "should include arguments"
        );
    }

    #[test]
    fn format_recent_message_includes_tool_call_id() {
        let rm = RecentMessage {
            message: Message::tool("file contents", "call_abc"),
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::User,
        };

        let formatted = format_recent_message(&rm);
        assert!(
            formatted.contains("(call: call_abc)"),
            "should include tool call id in header"
        );
    }

    #[test]
    fn format_recent_message_includes_timestamp() {
        let timestamp = chrono::NaiveDate::from_ymd_opt(2026, 2, 21)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let rm = RecentMessage {
            message: Message::user("hello"),
            timestamp,
            visibility: Visibility::User,
        };

        let formatted = format_recent_message(&rm);
        assert!(
            formatted.contains("2026-02-21"),
            "should include ISO date in timestamp"
        );
        assert!(
            formatted.contains("visibility: user"),
            "should include visibility"
        );
    }

    #[test]
    fn update_config_changes_thresholds() {
        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 1000,
                cooldown_secs: 60,
                force_threshold_tokens: 5000,
                tz: chrono_tz::UTC,
                role_overrides: None,
            },
        );

        assert_eq!(observer.threshold_tokens(), 1000);
        assert_eq!(observer.cooldown_secs(), 60);
        assert_eq!(observer.force_threshold_tokens(), 5000);

        observer.update_config(ObserverConfig {
            threshold_tokens: 2000,
            cooldown_secs: 120,
            force_threshold_tokens: 10000,
            tz: chrono_tz::US::Eastern,
            role_overrides: None,
        });

        assert_eq!(observer.threshold_tokens(), 2000);
        assert_eq!(observer.cooldown_secs(), 120);
        assert_eq!(observer.force_threshold_tokens(), 10000);
        assert_eq!(observer.timezone(), chrono_tz::US::Eastern);
    }

    #[tokio::test]
    async fn swap_provider_changes_model() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.episodes_dir())
            .await
            .unwrap();
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
                ..ObserverConfig::default()
            },
        );

        let new_response = r#"{
            "observations": [
                {"content": "new provider obs", "timestamp": "2026-02-21T14:30", "visibility": "user"}
            ],
            "narrative": ""
        }"#;
        observer.swap_provider(Box::new(MockMemoryProvider::new(new_response)));

        let messages = make_recent_messages(5);
        let result = observer.extract(&messages, &layout).await.unwrap();
        assert_eq!(
            result.observations.len(),
            1,
            "should have 1 observation from new provider"
        );
        assert_eq!(
            result.observations.first().map(|o| o.content.as_str()),
            Some("new provider obs"),
            "content should come from new provider"
        );
    }

    #[tokio::test]
    async fn extract_returns_err_for_empty_messages() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig::default(),
        );

        let result = observer.extract(&[], &layout).await;
        assert!(
            result.is_err(),
            "extract with empty messages should return Err"
        );
    }

    #[test]
    fn parse_extraction_items_skips_missing_content() {
        let items = vec![
            serde_json::json!({"timestamp": "2026-02-21T14:30", "visibility": "user"}),
            serde_json::json!({"content": "valid obs", "timestamp": "2026-02-21T14:31", "visibility": "user"}),
        ];
        let results = parse_extraction_items(&items, chrono_tz::UTC);
        assert_eq!(results.len(), 1, "item missing content should be skipped");
        assert_eq!(
            results.first().map(|e| e.content.as_str()),
            Some("valid obs")
        );
    }
}
