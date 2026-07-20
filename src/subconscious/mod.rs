//! Subconscious: a small-LLM classifier that watches the main agent's
//! conversation and steers it when it drifts from its instructions.
//!
//! Runs in two places: concurrently during a turn (mid-turn watch, injecting
//! corrections via the interrupt channel) and once after a user-visible turn
//! completes (end-of-turn check for omissions like unsaved preferences).
//! Opt-in via `[subconscious]` in config.toml because of latency and token
//! cost.

mod learning;
mod parse;
mod prompt;
mod watch;

pub use learning::LearningState;
pub use watch::SubconsciousWatch;

use anyhow::Context;
use serde::Deserialize;

use crate::config::{
    DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS, DEFAULT_SUBCONSCIOUS_MAX_INTERVENTIONS,
    DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS,
};
use crate::models::{CompletionOptions, Message, ModelProvider, ResponseFormat};
use crate::workspace::layout::WorkspaceLayout;
use parse::parse_subconscious_response;
use prompt::{
    DEFAULT_SUBCONSCIOUS_PROMPT, build_eval_prompt, format_turn_transcript,
    subconscious_response_schema,
};

/// Which evaluation pass is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalPhase {
    /// The agent is mid-turn; look for instruction violations in progress.
    MidTurn,
    /// The turn has ended; look for omissions the agent should fix.
    EndOfTurn,
}

/// How urgently a finding needs the agent's attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Worth mentioning; injected as passive context for the next turn.
    Note,
    /// Needs correction now; steers the agent immediately.
    Act,
}

/// What kind of drift the classifier detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingKind {
    /// The agent acted against one of its instructions.
    Violation,
    /// The agent failed to do something it should have done.
    Omission,
}

impl FindingKind {
    /// Lowercase wire name, matching the classifier's JSON output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::Violation => "violation",
            FindingKind::Omission => "omission",
        }
    }
}

/// A single problem the classifier found in the transcript.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Whether this is a violation or an omission.
    pub kind: FindingKind,
    /// Whether the agent should be steered now or just informed.
    pub severity: Severity,
    /// The corrective guidance to give the agent.
    pub instruction: String,
}

/// The kind of learnable signal surfaced at end-of-turn triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LearnSignalType {
    /// A user preference, correction, expressed frustration, or working-style cue.
    Preference,
    /// The agent hit an error or obstacle and had to work around it or find a
    /// non-obvious solution.
    Recovery,
}

impl LearnSignalType {
    /// Lowercase wire name, matching the classifier's JSON output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LearnSignalType::Preference => "preference",
            LearnSignalType::Recovery => "recovery",
        }
    }
}

/// A durable learnable signal surfaced at end-of-turn triage.
///
/// Distinct from a [`Finding`]: a finding steers the current agent, while a
/// learn signal is a durable observation worth persisting, handed off to the
/// `learner` sub-agent. Only emitted at end-of-turn, never mid-turn.
#[derive(Debug, Clone)]
pub struct LearnSignal {
    /// One or two sentences capturing the signal.
    pub summary: String,
    /// Whether this is a preference or a recovery signal.
    pub signal_type: LearnSignalType,
}

/// The result of one classifier evaluation pass.
#[derive(Debug, Default)]
pub struct EvalOutcome {
    /// Corrective findings (violations/omissions) that steer the agent.
    pub findings: Vec<Finding>,
    /// Durable learnable signals to persist. Only populated at end-of-turn with
    /// learning enabled; empty otherwise.
    pub learnings: Vec<LearnSignal>,
}

/// Steering the mid-turn watch already applied during a single turn.
///
/// Filled in by the mid-turn watch as it runs and handed to the end-of-turn
/// pass, which uses it to act as a triage step — not repeating corrections it
/// already delivered and folding in queued notes — rather than a blind
/// re-classification of the same turn.
#[derive(Debug, Default, Clone)]
pub struct TurnScratch {
    /// Corrections the mid-turn watch injected into the agent this turn.
    pub applied_corrections: Vec<String>,
    /// Findings the mid-turn watch observed but did not act on (extra `act`
    /// findings beyond the cap, all `note` findings, and corrections that
    /// could not be delivered because the turn had already ended).
    pub queued_notes: Vec<Finding>,
}

impl TurnScratch {
    /// Whether the watch recorded any steering or notes this turn.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.applied_corrections.is_empty() && self.queued_notes.is_empty()
    }
}

/// Subconscious runtime configuration.
#[derive(Debug, Clone)]
pub struct SubconsciousConfig {
    /// Master switch; when false, `evaluate` is never called.
    pub enabled: bool,
    /// Whether the mid-turn watch runs during the tool loop.
    pub mid_turn: bool,
    /// Evaluate every N tool-loop iterations.
    pub every_n_iterations: usize,
    /// Maximum mid-turn corrections injected per turn.
    pub max_interventions_per_turn: usize,
    /// Token cap for the transcript sent to the classifier.
    pub max_transcript_tokens: usize,
    /// Whether the activity-triggered learning loop is enabled (learn findings
    /// plus learner sub-agent spawns).
    pub learning: bool,
    /// Per-role overrides for temperature and thinking.
    pub role_overrides: Option<crate::config::RoleOverrides>,
}

impl Default for SubconsciousConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mid_turn: true,
            every_n_iterations: DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS,
            max_interventions_per_turn: DEFAULT_SUBCONSCIOUS_MAX_INTERVENTIONS,
            max_transcript_tokens: DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS,
            learning: false,
            role_overrides: None,
        }
    }
}

/// The subconscious classifier: watches turn transcripts and reports drift.
pub struct Subconscious {
    provider: Box<dyn ModelProvider>,
    config: SubconsciousConfig,
    layout: WorkspaceLayout,
}

impl Subconscious {
    /// Build a subconscious from the resolved config, disabled when opted out.
    ///
    /// Provider construction failure logs an error and falls back to a
    /// disabled instance rather than failing startup — the main agent must
    /// keep working without its subconscious.
    #[must_use]
    pub fn build(
        cfg: &crate::config::Config,
        layout: &WorkspaceLayout,
        http: crate::models::SharedHttpClient,
    ) -> std::sync::Arc<Self> {
        let settings = &cfg.subconscious_settings;
        if !settings.enabled {
            return std::sync::Arc::new(Self::disabled(layout.clone()));
        }

        let provider = match crate::models::build_provider_chain(
            &cfg.subconscious,
            cfg.max_tokens,
            http,
            cfg.retry.clone(),
        ) {
            Ok(p) => p,
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
                max_interventions_per_turn: settings.max_interventions_per_turn,
                max_transcript_tokens: settings.max_transcript_tokens,
                learning: settings.learning,
                role_overrides: cfg.role_overrides.get("subconscious").cloned(),
            },
            layout.clone(),
        ))
    }

    /// Create a new subconscious with the given provider and config.
    #[must_use]
    pub fn new(
        provider: Box<dyn ModelProvider>,
        config: SubconsciousConfig,
        layout: WorkspaceLayout,
    ) -> Self {
        Self {
            provider,
            config,
            layout,
        }
    }

    /// Create a disabled subconscious that never evaluates.
    ///
    /// Uses a `NullProvider` and `enabled: false` so callers can hold a
    /// value unconditionally. Used when the feature is off or provider
    /// construction fails.
    #[must_use]
    pub fn disabled(layout: WorkspaceLayout) -> Self {
        Self {
            provider: Box::new(crate::models::null::NullProvider),
            config: SubconsciousConfig::default(),
            layout,
        }
    }

    /// Whether the subconscious is enabled at all.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Whether the mid-turn watch should run.
    #[must_use]
    pub fn mid_turn_enabled(&self) -> bool {
        self.config.enabled && self.config.mid_turn
    }

    /// Whether the activity-triggered learning loop is active.
    ///
    /// Requires both the subconscious and its learning path to be enabled; the
    /// learner sub-agent is only spawned from `learn` findings when this is true.
    #[must_use]
    pub fn learning_enabled(&self) -> bool {
        self.config.enabled && self.config.learning
    }

    /// Evaluate every N tool-loop iterations.
    #[must_use]
    pub fn every_n_iterations(&self) -> usize {
        self.config.every_n_iterations.max(1)
    }

    /// Maximum mid-turn corrections injected per turn.
    #[must_use]
    pub fn max_interventions_per_turn(&self) -> usize {
        self.config.max_interventions_per_turn
    }

    /// Classify a turn transcript against the agent's instructions.
    ///
    /// An empty result means the agent's behavior looks fine — the common
    /// case. Reads the policy and identity files fresh from disk each call
    /// so edits apply without restart.
    ///
    /// # Errors
    /// Returns an error if the LLM call fails or the response cannot be
    /// parsed.
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

        // Learnable signals are only surfaced at end-of-turn triage, and only
        // when learning is enabled — never mid-turn.
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
            max_tokens: Some(1024),
            response_format: ResponseFormat::JsonSchema {
                name: "subconscious_findings".to_string(),
                schema: subconscious_response_schema(include_learnings),
            },
            ..CompletionOptions::default()
        };

        let response = self
            .provider
            .complete(&messages, &[], &options)
            .await
            .context("subconscious LLM call failed")?;

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

    /// Load SUBCONSCIOUS.md from the workspace, falling back to the default.
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

    /// Load the agent's instruction files the classifier checks against.
    ///
    /// Missing files are skipped silently — a fresh workspace may not have
    /// all of them yet, and the classifier degrades gracefully.
    async fn load_identity_context(&self) -> String {
        let sources = [
            ("SOUL.md", self.layout.soul_md()),
            ("AGENTS.md", self.layout.agents_md()),
            ("USER.md", self.layout.user_md()),
            ("MEMORY.md", self.layout.memory_md()),
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
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::test_helpers::MockMemoryProvider;

    const EMPTY_RESPONSE: &str = r#"{"findings": []}"#;
    const ACT_RESPONSE: &str = r#"{
        "findings": [
            {"kind": "omission", "severity": "act", "instruction": "The user said they prefer bullet points; save this preference to MEMORY.md."}
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
            finding.instruction.contains("MEMORY.md"),
            "instruction should carry the corrective guidance"
        );
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

        // Learn signals are end-of-turn only; a mid-turn pass must drop them.
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

    #[tokio::test]
    async fn evaluate_empty_transcript_short_circuits() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        // NullProvider would error if called; the short-circuit must win.
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
