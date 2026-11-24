mod learning;
mod parse;
mod prompt;
mod watch;

pub use learning::LearningState;
pub use watch::SubconsciousWatch;

use anyhow::Context;
use serde::Deserialize;

use parse::parse_subconscious_response;
use prompt::{
    DEFAULT_SUBCONSCIOUS_PROMPT, build_eval_prompt, format_turn_transcript,
    subconscious_response_schema,
};
use residuum_config::{
    DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS, DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS,
    WorkspaceLayout,
};
use residuum_inference::{
    CompletionOptions, InferenceProvider, Message, ResponseFormat, UsageComponent, UsageRecorder,
};

const OUTPUT_LIMIT: u32 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalPhase {
    MidTurn,
    EndOfTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Note,
    Act,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingKind {
    Violation,
    Omission,
}

impl FindingKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::Violation => "violation",
            FindingKind::Omission => "omission",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub kind: FindingKind,
    pub severity: Severity,
    pub instruction: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LearnSignalType {
    Preference,
    Recovery,
}

impl LearnSignalType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LearnSignalType::Preference => "preference",
            LearnSignalType::Recovery => "recovery",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LearnSignal {
    pub summary: String,
    pub signal_type: LearnSignalType,
}

#[derive(Debug, Default)]
pub struct EvalOutcome {
    pub findings: Vec<Finding>,
    pub learnings: Vec<LearnSignal>,
}

#[derive(Debug, Default, Clone)]
pub struct TurnScratch {
    pub applied_corrections: Vec<String>,
    pub queued_notes: Vec<Finding>,
}

impl TurnScratch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.applied_corrections.is_empty() && self.queued_notes.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct SubconsciousConfig {
    pub enabled: bool,
    pub mid_turn: bool,
    pub every_n_iterations: usize,
    pub max_transcript_tokens: usize,
    pub learning: bool,
    pub role_overrides: Option<residuum_config::RoleOverrides>,
}

impl Default for SubconsciousConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mid_turn: true,
            every_n_iterations: DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS,
            max_transcript_tokens: DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS,
            learning: false,
            role_overrides: None,
        }
    }
}

pub struct Subconscious {
    provider: Box<dyn InferenceProvider>,
    config: SubconsciousConfig,
    layout: WorkspaceLayout,
}

impl Subconscious {
    #[must_use]
    pub fn build(
        cfg: &residuum_config::Config,
        layout: &WorkspaceLayout,
        http: residuum_inference::SharedHttpClient,
        publisher: residuum_bus::Publisher,
    ) -> std::sync::Arc<Self> {
        let settings = &cfg.subconscious_settings;
        if !settings.enabled {
            return std::sync::Arc::new(Self::disabled(layout.clone()));
        }

        let provider = match residuum_inference::build_provider_chain_with_notices(
            &cfg.subconscious,
            cfg.max_tokens,
            http,
            cfg.retry.clone(),
            std::sync::Arc::new(publisher),
            "the subconscious",
        ) {
            Ok((provider, dropped)) => {
                for fallback in &dropped {
                    tracing::warn!(
                        provider = %fallback.name,
                        error = %fallback.error,
                        "dropped an unbuildable fallback provider from the subconscious chain"
                    );
                }
                provider
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to build subconscious provider, subconscious disabled");
                return std::sync::Arc::new(Self::disabled(layout.clone()));
            }
        };

        std::sync::Arc::new(Self::new(
            provider,
            SubconsciousConfig {
                enabled: true,
                mid_turn: settings.mid_turn,
                every_n_iterations: settings.every_n_iterations,
                max_transcript_tokens: settings.max_transcript_tokens,
                learning: settings.learning,
                role_overrides: cfg.role_overrides.get("subconscious").cloned(),
            },
            layout.clone(),
        ))
    }

    #[must_use]
    pub fn new(
        provider: Box<dyn InferenceProvider>,
        config: SubconsciousConfig,
        layout: WorkspaceLayout,
    ) -> Self {
        Self {
            provider,
            config,
            layout,
        }
    }

    #[must_use]
    pub fn disabled(layout: WorkspaceLayout) -> Self {
        Self {
            provider: Box::new(residuum_inference::NullProvider),
            config: SubconsciousConfig::default(),
            layout,
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    #[must_use]
    pub fn mid_turn_enabled(&self) -> bool {
        self.config.enabled && self.config.mid_turn
    }

    #[must_use]
    pub fn learning_enabled(&self) -> bool {
        self.config.enabled && self.config.learning
    }

    #[must_use]
    pub fn every_n_iterations(&self) -> usize {
        self.config.every_n_iterations.max(1)
    }

    #[tracing::instrument(skip_all, fields(operation = "subconscious_evaluate", phase = ?phase, message_count = transcript.len()))]
    pub async fn evaluate(
        &self,
        transcript: &[Message],
        phase: EvalPhase,
        prior: Option<&TurnScratch>,
    ) -> anyhow::Result<EvalOutcome> {
        if transcript.is_empty() {
            return Ok(EvalOutcome::default());
        }

        let include_learnings = self.config.learning && phase == EvalPhase::EndOfTurn;

        let policy = self.load_policy().await;
        let identity = self.load_identity_context().await;
        let transcript_text = format_turn_transcript(transcript, self.config.max_transcript_tokens);
        let messages = build_eval_prompt(
            &policy,
            &identity,
            &transcript_text,
            phase,
            prior,
            include_learnings,
        );

        let ov = self.config.role_overrides.as_ref();
        let options = CompletionOptions {
            temperature: ov.and_then(|o| o.temperature),
            thinking: ov.and_then(|o| o.thinking.clone()),
            max_tokens: Some(OUTPUT_LIMIT),
            response_format: ResponseFormat::JsonSchema {
                name: "subconscious_findings".to_string(),
                schema: subconscious_response_schema(include_learnings),
            },
            ..CompletionOptions::default()
        };

        let result = self.provider.complete(&messages, &[], &options).await;
        UsageRecorder::new(&self.layout)
            .record_call(
                &UsageComponent::Subconscious,
                None,
                &*self.provider,
                &result,
            )
            .await;
        let response = result.context("subconscious LLM call failed")?;
        if response.was_truncated() && response.content.trim().is_empty() {
            anyhow::bail!(
                "the subconscious model reached its {OUTPUT_LIMIT}-token output limit before answering, so this evaluation has no result; a model that reasons by default can spend the whole limit reasoning"
            );
        }

        let outcome = parse_subconscious_response(&response, include_learnings)?;
        if outcome.findings.is_empty() && outcome.learnings.is_empty() {
            tracing::debug!("subconscious found no issues");
        } else {
            tracing::info!(
                findings = outcome.findings.len(),
                learnings = outcome.learnings.len(),
                "subconscious reported results"
            );
        }
        Ok(outcome)
    }

    async fn load_policy(&self) -> String {
        match tokio::fs::read_to_string(self.layout.subconscious_md()).await {
            Ok(s) if !s.trim().is_empty() => s,
            Ok(_) => DEFAULT_SUBCONSCIOUS_PROMPT.to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                DEFAULT_SUBCONSCIOUS_PROMPT.to_string()
            }
            Err(e) => {
                tracing::warn!(path = %self.layout.subconscious_md().display(), error = %e, "failed to read subconscious policy, using default");
                DEFAULT_SUBCONSCIOUS_PROMPT.to_string()
            }
        }
    }

    async fn load_identity_context(&self) -> String {
        let team = self.layout.team();
        let sources = [
            ("SOUL.md", self.layout.soul_md()),
            ("team/AGENTS.md", team.agents_md()),
            ("team/USER.md", team.user_md()),
            ("team/wiki/index.md", team.wiki_index_md()),
        ];

        let mut sections = Vec::new();
        for (name, path) in sources {
            match tokio::fs::read_to_string(&path).await {
                Ok(content) if !content.trim().is_empty() => {
                    sections.push(format!("## {name}\n\n{content}"));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to read identity file for subconscious");
                }
            }
        }
        sections.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_memory::testing::MockMemoryProvider;

    const EMPTY_RESPONSE: &str = r#"{"findings": []}"#;
    const ACT_RESPONSE: &str = r#"{
        "findings": [
            {"kind": "omission", "severity": "act", "instruction": "The user said they prefer bullet points; save this preference to the wiki."}
        ]
    }"#;

    fn make_transcript() -> Vec<Message> {
        vec![
            Message::user("From now on, always answer me in bullet points."),
            Message::assistant("Sure, I'll do that.".to_string(), None),
        ]
    }

    #[tokio::test]
    async fn evaluate_parses_findings() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::new(
            Box::new(MockMemoryProvider::new(ACT_RESPONSE)),
            SubconsciousConfig {
                enabled: true,
                ..SubconsciousConfig::default()
            },
            layout,
        );

        let findings = sub
            .evaluate(&make_transcript(), EvalPhase::EndOfTurn, None)
            .await
            .unwrap()
            .findings;
        assert_eq!(findings.len(), 1, "should have one finding");
        let finding = findings.first().unwrap();
        assert_eq!(finding.kind, FindingKind::Omission);
        assert_eq!(finding.severity, Severity::Act);
        assert!(
            finding.instruction.contains("wiki"),
            "instruction should carry the corrective guidance"
        );
    }

    #[tokio::test]
    async fn evaluate_records_its_model_call_as_subconscious_usage() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let usage = residuum_inference::Usage {
            uncached_input_tokens: Some(2_000),
            output_tokens: Some(80),
            ..residuum_inference::Usage::default()
        };
        let sub = Subconscious::new(
            Box::new(MockMemoryProvider::new(ACT_RESPONSE).with_usage(usage)),
            SubconsciousConfig {
                enabled: true,
                ..SubconsciousConfig::default()
            },
            layout.clone(),
        );
        sub.evaluate(&make_transcript(), EvalPhase::MidTurn, None)
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
        assert_eq!(record.component, UsageComponent::Subconscious);
        assert_eq!(record.model, "mock-memory");
        assert_eq!(record.usage, Some(usage));
    }

    #[tokio::test]
    async fn evaluate_surfaces_learnings_when_learning_enabled() {
        const LEARN_RESPONSE: &str = r#"{
            "findings": [],
            "learnings": [
                {"summary": "User prefers bullet points.", "signal_type": "preference"}
            ]
        }"#;
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::new(
            Box::new(MockMemoryProvider::new(LEARN_RESPONSE)),
            SubconsciousConfig {
                enabled: true,
                learning: true,
                ..SubconsciousConfig::default()
            },
            layout,
        );

        let outcome = sub
            .evaluate(&make_transcript(), EvalPhase::EndOfTurn, None)
            .await
            .unwrap();
        assert_eq!(outcome.learnings.len(), 1, "learn signal should parse");
        let signal = outcome.learnings.first().unwrap();
        assert_eq!(
            signal.signal_type,
            crate::subconscious::LearnSignalType::Preference
        );
        assert!(signal.summary.contains("bullet points"));
    }

    #[tokio::test]
    async fn evaluate_drops_learnings_mid_turn() {
        const LEARN_RESPONSE: &str = r#"{
            "findings": [],
            "learnings": [
                {"summary": "User prefers bullet points.", "signal_type": "preference"}
            ]
        }"#;
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::new(
            Box::new(MockMemoryProvider::new(LEARN_RESPONSE)),
            SubconsciousConfig {
                enabled: true,
                learning: true,
                ..SubconsciousConfig::default()
            },
            layout,
        );

        let outcome = sub
            .evaluate(&make_transcript(), EvalPhase::MidTurn, None)
            .await
            .unwrap();
        assert!(
            outcome.learnings.is_empty(),
            "learn signals must never surface mid-turn"
        );
    }

    #[tokio::test]
    async fn evaluate_empty_findings_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::new(
            Box::new(MockMemoryProvider::new(EMPTY_RESPONSE)),
            SubconsciousConfig {
                enabled: true,
                ..SubconsciousConfig::default()
            },
            layout,
        );

        let outcome = sub
            .evaluate(&make_transcript(), EvalPhase::MidTurn, None)
            .await
            .unwrap();
        assert!(
            outcome.findings.is_empty(),
            "empty findings should parse to empty"
        );
    }

    struct ReasonedOutOfTokens;

    #[async_trait::async_trait]
    impl InferenceProvider for ReasonedOutOfTokens {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            let mut response = residuum_inference::InferenceResponse::new(String::new(), vec![]);
            response.stop_reason = Some(residuum_inference::StopReason::MaxTokens);
            Ok(response)
        }

        fn model_name(&self) -> &'static str {
            "reasoned-out-of-tokens"
        }
    }

    #[tokio::test]
    async fn an_empty_answer_cut_off_by_the_output_limit_is_a_failed_evaluation() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::new(
            Box::new(ReasonedOutOfTokens),
            SubconsciousConfig {
                enabled: true,
                ..SubconsciousConfig::default()
            },
            layout.clone(),
        );

        let error = sub
            .evaluate(&make_transcript(), EvalPhase::MidTurn, None)
            .await
            .expect_err("an evaluation with no answer must not read as no findings");

        assert!(
            error.to_string().contains("output limit before answering"),
            "the error names the cause: {error}"
        );
        let records = UsageRecorder::new(&layout)
            .load(
                chrono::DateTime::<chrono::Utc>::MIN_UTC,
                chrono::DateTime::<chrono::Utc>::MAX_UTC,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            records.len(),
            1,
            "the call that spent its limit reasoning is still recorded"
        );
    }

    #[tokio::test]
    async fn evaluate_empty_transcript_short_circuits() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::disabled(layout);

        let outcome = sub.evaluate(&[], EvalPhase::EndOfTurn, None).await.unwrap();
        assert!(
            outcome.findings.is_empty() && outcome.learnings.is_empty(),
            "empty transcript should return empty"
        );
    }

    #[tokio::test]
    async fn evaluate_reads_policy_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::write(layout.subconscious_md(), "Custom policy: flag everything.")
            .await
            .unwrap();

        let sub = Subconscious::new(
            Box::new(MockMemoryProvider::new(EMPTY_RESPONSE)),
            SubconsciousConfig {
                enabled: true,
                ..SubconsciousConfig::default()
            },
            layout,
        );
        assert_eq!(sub.load_policy().await, "Custom policy: flag everything.");
    }

    #[tokio::test]
    async fn load_policy_falls_back_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::disabled(layout);

        assert_eq!(sub.load_policy().await, DEFAULT_SUBCONSCIOUS_PROMPT);
    }

    #[tokio::test]
    async fn load_identity_context_skips_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::write(layout.soul_md(), "Be kind.")
            .await
            .unwrap();

        let sub = Subconscious::disabled(layout);
        let identity = sub.load_identity_context().await;
        assert!(identity.contains("## SOUL.md"), "present file included");
        assert!(identity.contains("Be kind."), "content included");
        assert!(
            !identity.contains("AGENTS.md"),
            "missing files should be skipped"
        );
    }

    #[tokio::test]
    async fn load_identity_context_reads_team_files() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("scout"));
        let team = layout.team();
        tokio::fs::create_dir_all(layout.root()).await.unwrap();
        tokio::fs::create_dir_all(team.wiki_dir()).await.unwrap();
        tokio::fs::write(layout.soul_md(), "Be kind.")
            .await
            .unwrap();
        tokio::fs::write(team.agents_md(), "Team rule.")
            .await
            .unwrap();
        tokio::fs::write(team.user_md(), "Sam likes tea.")
            .await
            .unwrap();
        tokio::fs::write(team.wiki_index_md(), "Wiki catalog.")
            .await
            .unwrap();
        tokio::fs::write(layout.root().join("USER.md"), "stale agent copy")
            .await
            .unwrap();

        let sub = Subconscious::disabled(layout);
        let identity = sub.load_identity_context().await;
        assert!(identity.contains("Team rule."), "team AGENTS.md included");
        assert!(identity.contains("Sam likes tea."), "team USER.md included");
        assert!(
            identity.contains("Wiki catalog."),
            "team wiki index included"
        );
        assert!(
            !identity.contains("stale agent copy"),
            "agent-dir USER.md is not read"
        );
    }

    #[test]
    fn disabled_reports_disabled() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        let sub = Subconscious::disabled(layout);
        assert!(!sub.enabled(), "disabled() should not be enabled");
        assert!(!sub.mid_turn_enabled(), "mid-turn should also be off");
    }

    #[test]
    fn mid_turn_requires_enabled() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        let sub = Subconscious::new(
            Box::new(MockMemoryProvider::new(EMPTY_RESPONSE)),
            SubconsciousConfig {
                enabled: false,
                mid_turn: true,
                ..SubconsciousConfig::default()
            },
            layout,
        );
        assert!(
            !sub.mid_turn_enabled(),
            "mid_turn without enabled should be off"
        );
    }

    #[test]
    fn every_n_iterations_is_never_zero() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        let sub = Subconscious::new(
            Box::new(MockMemoryProvider::new(EMPTY_RESPONSE)),
            SubconsciousConfig {
                every_n_iterations: 0,
                ..SubconsciousConfig::default()
            },
            layout,
        );
        assert_eq!(sub.every_n_iterations(), 1, "zero cadence clamps to 1");
    }
}
