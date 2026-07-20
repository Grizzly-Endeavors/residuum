//! Activity-triggered learning loop: decides when to spawn the `learner`
//! sub-agent and builds its spawn request.
//!
//! Two triggers feed one spawn path:
//! - the subconscious surfaces `learn` signals at end-of-turn triage, or
//! - a dumb turn-count fallback fires for users running with the subconscious
//!   off.
//!
//! Both share a single cooldown so the learner never spawns more than once per
//! window. All state here is in-memory and resets on gateway restart.

use std::time::{Duration, Instant};

use crate::bus::{EventTrigger, PresetName, SpawnRequestEvent};

use super::LearnSignal;

/// Preset name of the learner sub-agent.
const LEARNER_PRESET: &str = "learner";

/// In-memory state for the learning loop.
///
/// Not persisted: the cooldown anchor and turn counter reset on gateway
/// restart, so a fresh process may spawn the learner sooner than the configured
/// cooldown would otherwise allow.
#[derive(Debug, Default)]
pub struct LearningState {
    /// When the last learner spawn fired (cooldown anchor).
    last_spawn: Option<Instant>,
    /// Completed foreground turns since the last spawn (fallback trigger).
    turns_since_spawn: u32,
}

impl LearningState {
    /// Whether the cooldown window has elapsed since the last spawn.
    fn cooldown_ok(&self, cooldown: Duration, now: Instant) -> bool {
        match self.last_spawn {
            None => true,
            Some(last) => now.duration_since(last) >= cooldown,
        }
    }

    /// Record that a learner spawn just fired: reset the cooldown anchor and
    /// the fallback turn counter.
    fn mark_spawned(&mut self, now: Instant) {
        self.last_spawn = Some(now);
        self.turns_since_spawn = 0;
    }

    /// Handle `learn` signals from the subconscious end-of-turn triage.
    ///
    /// Returns a spawn request for the `learner` preset when the cooldown
    /// allows, batching every signal summary into one prompt. Empty input or a
    /// live cooldown yields `None`. Logs the spawn decision at debug level.
    pub fn on_learn_signals(
        &mut self,
        signals: &[LearnSignal],
        cooldown: Duration,
        now: Instant,
    ) -> Option<SpawnRequestEvent> {
        if signals.is_empty() {
            return None;
        }
        if !self.cooldown_ok(cooldown, now) {
            tracing::debug!(
                signals = signals.len(),
                decision = "suppressed_by_cooldown",
                "learner spawn suppressed"
            );
            return None;
        }
        self.mark_spawned(now);
        tracing::debug!(
            signals = signals.len(),
            decision = "fired",
            "learner spawn fired from subconscious signals"
        );
        Some(build_signal_spawn(signals))
    }

    /// Count a completed foreground turn on the fallback path and spawn the
    /// learner once `nudge_after_turns` turns have elapsed since the last spawn.
    ///
    /// Used only when the subconscious learning path is inactive.
    /// `nudge_after_turns == 0` disables the fallback. Respects the same
    /// cooldown as the signal path; a suppressed spawn keeps the counter armed
    /// so it retries on the next turn. Logs the spawn decision at debug level.
    pub fn on_turn_completed(
        &mut self,
        nudge_after_turns: u32,
        cooldown: Duration,
        now: Instant,
    ) -> Option<SpawnRequestEvent> {
        if nudge_after_turns == 0 {
            return None;
        }
        self.turns_since_spawn = self.turns_since_spawn.saturating_add(1);
        if self.turns_since_spawn < nudge_after_turns {
            return None;
        }
        if !self.cooldown_ok(cooldown, now) {
            tracing::debug!(
                turns = self.turns_since_spawn,
                decision = "suppressed_by_cooldown",
                "learner nudge suppressed"
            );
            return None;
        }
        self.mark_spawned(now);
        tracing::debug!(decision = "fired", "learner nudge fired from turn count");
        Some(build_nudge_spawn())
    }
}

/// Build the batched spawn request for subconscious-detected learn signals.
fn build_signal_spawn(signals: &[LearnSignal]) -> SpawnRequestEvent {
    let mut prompt = String::from(
        "The subconscious flagged learnable signals from the recent conversation:\n\n",
    );
    for signal in signals {
        prompt.push_str("- [");
        prompt.push_str(signal.signal_type.as_str());
        prompt.push_str("] ");
        prompt.push_str(&signal.summary);
        prompt.push('\n');
    }
    prompt.push_str(
        "\nThe full recent transcript is available in the workspace file recent_messages.json. \
         Review these signals, corroborate against memory, and persist per your instructions.",
    );
    spawn_event("learning:subconscious", prompt)
}

/// Build the generic spawn request for the turn-count fallback trigger.
fn build_nudge_spawn() -> SpawnRequestEvent {
    spawn_event(
        "learning:nudge",
        "Review the recent conversation in recent_messages.json for anything worth learning — \
         user preferences, corrections, recovery from errors — corroborate against memory, and \
         persist per your instructions."
            .to_string(),
    )
}

/// Assemble a learner `SpawnRequestEvent`.
///
/// No `model_tier_override` is set, so the `learner` preset's own `model_tier`
/// frontmatter resolves at spawn time.
fn spawn_event(source_label: &str, prompt: String) -> SpawnRequestEvent {
    SpawnRequestEvent {
        preset: PresetName::from(LEARNER_PRESET),
        source_label: source_label.to_string(),
        prompt,
        context: None,
        source: EventTrigger::Agent,
        model_tier_override: None,
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::subconscious::LearnSignalType;

    fn signal(summary: &str, signal_type: LearnSignalType) -> LearnSignal {
        LearnSignal {
            summary: summary.to_string(),
            signal_type,
        }
    }

    const COOLDOWN: Duration = Duration::from_secs(240 * 60);

    #[test]
    fn learn_signals_fire_then_cooldown_suppresses() {
        let mut state = LearningState::default();
        let now = Instant::now();
        let signals = vec![
            signal("User prefers bullets.", LearnSignalType::Preference),
            signal("Worked around a 502.", LearnSignalType::Recovery),
        ];

        // First call fires and batches both summaries into one prompt.
        let spawn = state.on_learn_signals(&signals, COOLDOWN, now).unwrap();
        assert_eq!(spawn.preset.as_ref(), "learner");
        assert_eq!(spawn.source_label, "learning:subconscious");
        assert!(spawn.prompt.contains("User prefers bullets."));
        assert!(spawn.prompt.contains("Worked around a 502."));
        assert!(spawn.prompt.contains("recent_messages.json"));
        assert!(spawn.model_tier_override.is_none());

        // A second call inside the window is suppressed.
        assert!(
            state.on_learn_signals(&signals, COOLDOWN, now).is_none(),
            "cooldown should suppress the second spawn"
        );

        // Once the window elapses, it fires again.
        let later = now.checked_add(COOLDOWN).unwrap();
        assert!(
            state.on_learn_signals(&signals, COOLDOWN, later).is_some(),
            "spawn should fire again after the cooldown elapses"
        );
    }

    #[test]
    fn empty_signals_never_spawn() {
        let mut state = LearningState::default();
        assert!(
            state
                .on_learn_signals(&[], COOLDOWN, Instant::now())
                .is_none()
        );
    }

    #[test]
    fn nudge_disabled_when_zero() {
        let mut state = LearningState::default();
        let now = Instant::now();
        for _ in 0..10 {
            assert!(
                state.on_turn_completed(0, COOLDOWN, now).is_none(),
                "nudge_after_turns=0 disables the fallback"
            );
        }
    }

    #[test]
    fn nudge_fires_after_threshold_turns() {
        let mut state = LearningState::default();
        let now = Instant::now();

        // Turns 1 and 2 do not reach the threshold of 3.
        assert!(state.on_turn_completed(3, COOLDOWN, now).is_none());
        assert!(state.on_turn_completed(3, COOLDOWN, now).is_none());
        // Third turn fires the nudge.
        let spawn = state.on_turn_completed(3, COOLDOWN, now).unwrap();
        assert_eq!(spawn.source_label, "learning:nudge");
        assert!(spawn.prompt.contains("recent_messages.json"));

        // Counter reset — it takes another 3 turns to fire again (and cooldown
        // must also have elapsed).
        let later = now.checked_add(COOLDOWN).unwrap();
        assert!(state.on_turn_completed(3, COOLDOWN, later).is_none());
        assert!(state.on_turn_completed(3, COOLDOWN, later).is_none());
        assert!(state.on_turn_completed(3, COOLDOWN, later).is_some());
    }

    #[test]
    fn nudge_respects_cooldown_and_retries() {
        let mut state = LearningState::default();
        let now = Instant::now();

        // Fire a spawn via signals to arm the cooldown.
        state
            .on_learn_signals(&[signal("x", LearnSignalType::Preference)], COOLDOWN, now)
            .unwrap();

        // Threshold reached but cooldown still active → suppressed, counter stays armed.
        assert!(state.on_turn_completed(1, COOLDOWN, now).is_none());
        assert!(state.on_turn_completed(1, COOLDOWN, now).is_none());

        // Once the window elapses, the still-armed counter fires.
        let later = now.checked_add(COOLDOWN).unwrap();
        assert!(state.on_turn_completed(1, COOLDOWN, later).is_some());
    }
}
