//! Activity-triggered learning loop: decides when to spawn the `learner`
//! sub-agent and builds its spawn request.
//!
//! Two triggers feed one spawn path:
//! - the subconscious surfaces `learn` signals at end-of-turn triage, or
//! - a dumb turn-count fallback fires for users running with the subconscious
//!   off.
//!
//! Both share a single cooldown so the learner never spawns more than once per
//! window. A signal that arrives while the cooldown is active is queued
//! rather than dropped, and carried into whichever trigger next clears the
//! cooldown. All state here is in-memory and resets on gateway restart.

use std::time::{Duration, Instant};

use crate::background::registry::{MAIN_ADDRESS, MAIN_DEPTH, generate_address};
use crate::bus::{EventTrigger, SessionAddress, SkillName, SpawnRequestEvent};

use super::LearnSignal;

/// Preset name of the learner sub-agent.
const LEARNER_SKILL: &str = "learner";

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
    /// Learn signals that arrived during an active cooldown. Carried into
    /// whichever trigger — signals or the turn-count fallback — next clears
    /// the cooldown, rather than being dropped.
    pending_signals: Vec<LearnSignal>,
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
    /// Returns a spawn request for the `learner` skill when the cooldown
    /// allows, batching every signal summary into one prompt — including any
    /// signals queued from an earlier call suppressed by the cooldown. Empty
    /// input yields `None`. A live cooldown queues the signals for the next
    /// spawn (from either trigger) instead of dropping them, and yields
    /// `None`.
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
            self.pending_signals.extend_from_slice(signals);
            tracing::debug!(
                signals = signals.len(),
                queued_total = self.pending_signals.len(),
                decision = "queued_by_cooldown",
                "learner spawn suppressed by cooldown; signals queued for the next spawn"
            );
            return None;
        }
        self.mark_spawned(now);
        let mut batch = std::mem::take(&mut self.pending_signals);
        batch.extend_from_slice(signals);
        tracing::debug!(
            signals = batch.len(),
            decision = "fired",
            "learner spawn fired from subconscious signals"
        );
        Some(build_signal_spawn(&batch))
    }

    /// Count a completed foreground turn on the fallback path and spawn the
    /// learner once `nudge_after_turns` turns have elapsed since the last spawn.
    ///
    /// Used only when the subconscious learning path is inactive.
    /// `nudge_after_turns == 0` disables the fallback. Respects the same
    /// cooldown as the signal path; a suppressed spawn keeps the counter armed
    /// so it retries on the next turn. If signals queued during the cooldown
    /// are waiting, this fires the batched signal prompt instead of the
    /// generic nudge, so they aren't dropped just because the fallback
    /// trigger reached threshold first. Logs the spawn decision at debug level.
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
        if self.pending_signals.is_empty() {
            tracing::debug!(decision = "fired", "learner nudge fired from turn count");
            Some(build_nudge_spawn())
        } else {
            let batch = std::mem::take(&mut self.pending_signals);
            tracing::debug!(
                signals = batch.len(),
                decision = "fired_with_queued_signals",
                "learner nudge fired, carrying signals queued during cooldown"
            );
            Some(build_signal_spawn(&batch))
        }
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
/// The learner reasons about the user from the live transcript, so it runs on
/// the large tier. Every fork carries the agent's own identity in its system
/// message now, so no special flag is needed for that.
fn spawn_event(source_label: &str, prompt: String) -> SpawnRequestEvent {
    let trigger = EventTrigger::Agent;
    let address = generate_address(&trigger, LEARNER_SKILL);
    SpawnRequestEvent {
        address,
        skill: Some(SkillName::from(LEARNER_SKILL)),
        source_label: source_label.to_string(),
        prompt,
        context: None,
        source: trigger,
        model_tier: crate::config::BackgroundModelTier::Large,
        // The learner is part of the main agent's own subconscious, not
        // something a session spawned, so it runs at main's own spawn depth.
        spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
        depth: MAIN_DEPTH + 1,
        // The learner is triggered by the gateway event loop's own
        // turn-count/signal logic, not from within a live turn of main's —
        // there is no "spawning turn" whose hop count it could carry one
        // more than, so it starts a fresh chain at hop 0, the same as a
        // pulse or scheduled action.
        hop_count: 0,
        sender: None,
        conversation: None,
        inbound: None,
        images: Vec::new(),
        overlap: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subconscious::LearnSignalType;

    fn signal(summary: &str, signal_type: LearnSignalType) -> LearnSignal {
        LearnSignal {
            summary: summary.to_string(),
            signal_type,
        }
    }

    const COOLDOWN: Duration = Duration::from_hours(4);

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
        assert_eq!(spawn.skill.as_ref().map(AsRef::as_ref), Some("learner"));
        assert_eq!(spawn.source_label, "learning:subconscious");
        assert!(spawn.prompt.contains("User prefers bullets."));
        assert!(spawn.prompt.contains("Worked around a 502."));
        assert!(spawn.prompt.contains("recent_messages.json"));
        assert!(matches!(
            spawn.model_tier,
            crate::config::BackgroundModelTier::Large
        ));
        assert!(spawn.address.as_ref().starts_with("spawned-learner-"));

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

    #[test]
    fn signals_suppressed_by_cooldown_are_queued_into_next_spawn() {
        let mut state = LearningState::default();
        let now = Instant::now();

        // First call fires and arms the cooldown.
        state
            .on_learn_signals(
                &[signal("first", LearnSignalType::Preference)],
                COOLDOWN,
                now,
            )
            .unwrap();

        // A second call inside the window is suppressed but must not drop
        // its signal — it should be queued for the next spawn.
        assert!(
            state
                .on_learn_signals(
                    &[signal("queued", LearnSignalType::Recovery)],
                    COOLDOWN,
                    now
                )
                .is_none(),
            "cooldown should suppress the second spawn"
        );

        // Once the cooldown elapses, the next spawn carries the queued signal.
        let later = now.checked_add(COOLDOWN).unwrap();
        let spawn = state
            .on_learn_signals(
                &[signal("new", LearnSignalType::Preference)],
                COOLDOWN,
                later,
            )
            .unwrap();
        assert!(
            spawn.prompt.contains("queued"),
            "the signal queued during cooldown should not be dropped: {}",
            spawn.prompt
        );
        assert!(
            spawn.prompt.contains("new"),
            "the signal that triggered the new spawn should also be included: {}",
            spawn.prompt
        );
    }

    #[test]
    fn nudge_flushes_signals_queued_during_cooldown() {
        let mut state = LearningState::default();
        let now = Instant::now();

        // Fire a spawn via signals to arm the cooldown.
        state
            .on_learn_signals(
                &[signal("armed", LearnSignalType::Preference)],
                COOLDOWN,
                now,
            )
            .unwrap();

        // A signal arriving during the cooldown is queued, not dropped.
        assert!(
            state
                .on_learn_signals(
                    &[signal("queued-for-nudge", LearnSignalType::Recovery)],
                    COOLDOWN,
                    now
                )
                .is_none()
        );

        // Once the cooldown elapses, the turn-count fallback fires first and
        // must carry the queued signal instead of the generic nudge prompt.
        let later = now.checked_add(COOLDOWN).unwrap();
        let spawn = state.on_turn_completed(1, COOLDOWN, later).unwrap();
        assert_eq!(
            spawn.source_label, "learning:subconscious",
            "a nudge with queued signals should use the signal-batch spawn, not the generic nudge"
        );
        assert!(spawn.prompt.contains("queued-for-nudge"));
    }
}
