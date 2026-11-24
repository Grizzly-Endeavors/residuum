mod parse;
mod prompt;

use std::sync::Arc;

use anyhow::Context;
use chrono::NaiveDateTime;
use chrono_tz::Tz;

use crate::recent_messages::RecentMessage;
use crate::types::Visibility;
use parse::parse_observer_response;
use prompt::{EXTRACTION_CONTENT_PROMPT, build_extraction_prompt, observer_response_schema};
use residuum_config::{
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
    WorkspaceLayout,
};
use residuum_inference::{
    CompletionOptions, InferenceProvider, ResponseFormat, UsageComponent, UsageRecorder,
};

#[derive(Debug, Clone)]
pub struct ExtractedObservation {
    pub timestamp: NaiveDateTime,
    pub visibility: Visibility,
    pub content: String,
}

pub struct Extraction {
    pub narrative: Option<String>,
    pub observations: Vec<ExtractedObservation>,
    pub messages: Vec<RecentMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserveAction {
    None,
    StartCooldown,
    ForceNow,
}

#[derive(Debug, Clone)]
pub struct ObserverConfig {
    pub threshold_tokens: usize,
    pub cooldown_secs: u64,
    pub force_threshold_tokens: usize,
    pub tz: Tz,
    pub role_overrides: Option<residuum_config::RoleOverrides>,
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

struct ObserverInner {
    provider: Arc<dyn InferenceProvider>,
    config: ObserverConfig,
}

pub struct Observer {
    inner: std::sync::RwLock<ObserverInner>,
    automatic_failure: residuum_core::util::BackoffTracker,
}

impl Observer {
    #[must_use]
    pub fn new(provider: Box<dyn InferenceProvider>, config: ObserverConfig) -> Self {
        Self {
            inner: std::sync::RwLock::new(ObserverInner {
                provider: Arc::from(provider),
                config,
            }),
            automatic_failure: residuum_core::util::BackoffTracker::new(),
        }
    }

    #[must_use]
    pub fn disabled(tz: Tz) -> Self {
        Self {
            inner: std::sync::RwLock::new(ObserverInner {
                provider: Arc::new(residuum_inference::NullProvider),
                config: ObserverConfig {
                    threshold_tokens: usize::MAX,
                    cooldown_secs: u64::MAX,
                    force_threshold_tokens: usize::MAX,
                    tz,
                    role_overrides: None,
                },
            }),
            automatic_failure: residuum_core::util::BackoffTracker::new(),
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

    #[must_use]
    pub fn cooldown_secs(&self) -> u64 {
        self.read_inner().config.cooldown_secs
    }

    #[must_use]
    pub fn threshold_tokens(&self) -> usize {
        self.read_inner().config.threshold_tokens
    }

    #[must_use]
    pub fn force_threshold_tokens(&self) -> usize {
        self.read_inner().config.force_threshold_tokens
    }

    #[must_use]
    pub fn timezone(&self) -> Tz {
        self.read_inner().config.tz
    }

    #[must_use]
    pub fn automatic_failure_tracker(&self) -> &residuum_core::util::BackoffTracker {
        &self.automatic_failure
    }

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

    pub fn swap_provider(&self, provider: Box<dyn InferenceProvider>) {
        tracing::debug!("swapping observer model provider");
        self.write_inner().provider = Arc::from(provider);
        self.automatic_failure.clear_backoff_wait();
    }

    pub fn replace_from(&self, other: Self) {
        tracing::debug!("replacing observer provider and config in place");
        let new_inner = other
            .inner
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *self.write_inner() = new_inner;
        self.automatic_failure.clear_backoff_wait();
    }

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

    #[tracing::instrument(skip_all, fields(operation = "extract", message_count = recent_messages.len()))]
    pub async fn extract(
        &self,
        recent_messages: &[RecentMessage],
        layout: &WorkspaceLayout,
        conversation: Option<&str>,
    ) -> anyhow::Result<Extraction> {
        if recent_messages.is_empty() {
            anyhow::bail!("no recent messages to extract from");
        }

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

        let extraction_messages = build_extraction_prompt(recent_messages, &content_guidance);

        let (provider, role_overrides, tz) = {
            let inner = self.read_inner();
            (
                Arc::clone(&inner.provider),
                inner.config.role_overrides.clone(),
                inner.config.tz,
            )
        };

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
        let result = provider.complete(&extraction_messages, &[], &options).await;
        UsageRecorder::new(layout)
            .record_call(&UsageComponent::Observer, conversation, &*provider, &result)
            .await;
        let response = result.context("observer LLM call failed")?;

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

fn estimate_recent_tokens(recent_messages: &[RecentMessage]) -> usize {
    recent_messages
        .iter()
        .map(|rm| crate::tokens::estimate_single_message(&rm.message))
        .sum()
}

#[must_use]
pub fn extraction_tokens(rm: &RecentMessage) -> usize {
    crate::tokens::estimate_tokens(&prompt::format_recent_message(rm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recent_messages::RecentMessage;
    use crate::testing::MockMemoryProvider;
    use crate::types::Visibility;
    use parse::{parse_extraction_items, parse_observer_response};
    use prompt::{
        EXTRACTION_CONTENT_PROMPT, EXTRACTION_FORMAT_SPEC, build_extraction_prompt,
        format_recent_message,
    };
    use residuum_inference::{InferenceResponse, Message, Role};

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
                turn_id: None,
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
        let result = observer.extract(&messages, &layout, None).await.unwrap();

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

    #[tokio::test]
    async fn extract_records_its_model_call_as_observer_usage() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let usage = residuum_inference::Usage {
            uncached_input_tokens: Some(900),
            output_tokens: Some(60),
            ..residuum_inference::Usage::default()
        };
        let observer = Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE).with_usage(usage)),
            ObserverConfig::default(),
        );

        observer
            .extract(&make_recent_messages(3), &layout, Some("run-7"))
            .await
            .unwrap();

        let records = UsageRecorder::new(&layout)
            .load(
                chrono::DateTime::<chrono::Utc>::MIN_UTC,
                chrono::DateTime::<chrono::Utc>::MAX_UTC,
                None,
            )
            .await
            .unwrap();
        assert_eq!(records.len(), 1, "one record for the one call");
        let record = records.first().unwrap();
        assert_eq!(record.component, UsageComponent::Observer);
        assert_eq!(record.conversation.as_deref(), Some("run-7"));
        assert_eq!(record.model, "mock-memory");
        assert_eq!(record.usage, Some(usage));
    }

    #[test]
    fn extraction_prompt_includes_messages() {
        let recent_messages = vec![RecentMessage {
            message: Message::user("test content"),
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::User,
            turn_id: None,
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
        use residuum_inference::ToolCall;

        let rm = RecentMessage {
            message: Message::assistant(
                String::new(),
                Some(vec![ToolCall {
                    id: "call_abc".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                    server: None,
                }]),
            ),
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::User,
            turn_id: None,
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
            turn_id: None,
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
            turn_id: None,
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
        let result = observer.extract(&messages, &layout, None).await.unwrap();
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
    async fn replace_from_updates_shared_arc_in_place_and_clears_backoff_wait() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.episodes_dir())
            .await
            .unwrap();
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let observer = Arc::new(Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: 10,
                ..ObserverConfig::default()
            },
        ));
        let held_by_worker = Arc::clone(&observer);

        assert_eq!(
            observer.automatic_failure_tracker().record_failure(),
            residuum_core::util::NoticeAction::FailureStarted
        );
        assert_eq!(
            held_by_worker.automatic_failure_tracker().gate(),
            residuum_core::util::RetryGate::Skip
        );

        let new_response = r#"{
            "observations": [
                {"content": "reloaded observer obs", "timestamp": "2026-02-21T14:30", "visibility": "user"}
            ],
            "narrative": ""
        }"#;
        let replacement = Observer::new(
            Box::new(MockMemoryProvider::new(new_response)),
            ObserverConfig {
                threshold_tokens: 42,
                ..ObserverConfig::default()
            },
        );
        observer.replace_from(replacement);

        assert_eq!(held_by_worker.threshold_tokens(), 42);
        assert_eq!(
            held_by_worker.automatic_failure_tracker().gate(),
            residuum_core::util::RetryGate::Proceed,
            "replace_from should clear backoff wait on existing Arc clones"
        );
        let messages = make_recent_messages(5);
        let result = held_by_worker
            .extract(&messages, &layout, None)
            .await
            .unwrap();
        assert_eq!(
            result.observations.first().map(|o| o.content.as_str()),
            Some("reloaded observer obs")
        );
        assert_eq!(
            held_by_worker.automatic_failure_tracker().record_success(),
            residuum_core::util::NoticeAction::Recovered
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

        let result = observer.extract(&[], &layout, None).await;
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

    #[test]
    fn the_extraction_prompt_leaves_out_the_models_thinking() {
        let recent_messages = vec![RecentMessage {
            message: residuum_inference::Message::assistant("The answer is 42.".to_string(), None)
                .with_thinking(vec![residuum_inference::ThinkingBlock {
                    text: "private reasoning about the answer".to_string(),
                    signature: Some("sig-opaque-token".to_string()),
                    redacted: Some("encrypted-blob".to_string()),
                    part: None,
                    origin: None,
                    binding: None,
                }]),
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::User,
            turn_id: None,
        }];

        let prompt = build_extraction_prompt(&recent_messages, EXTRACTION_CONTENT_PROMPT);

        let everything: String = prompt
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            everything.contains("The answer is 42."),
            "the reply is still there"
        );
        for leaked in ["private reasoning", "sig-opaque-token", "encrypted-blob"] {
            assert!(
                !everything.contains(leaked),
                "{leaked} must not reach the observer: {everything}"
            );
        }
    }
}
