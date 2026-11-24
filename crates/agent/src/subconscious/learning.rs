use std::time::{Duration, Instant};

use residuum_bus::session::{MAIN_ADDRESS, MAIN_DEPTH, generate_address};
use residuum_bus::{EventTrigger, SessionAddress, SkillName, SpawnRequestEvent};

use super::LearnSignal;

const LEARNER_SKILL: &str = "learner";

#[derive(Debug, Default)]
pub struct LearningState {
    last_spawn: Option<Instant>,
    turns_since_spawn: u32,
    pending_signals: Vec<LearnSignal>,
}

impl LearningState {
    fn cooldown_ok(&self, cooldown: Duration, now: Instant) -> bool {
        match self.last_spawn {
            None => true,
            Some(last) => now.duration_since(last) >= cooldown,
        }
    }

    fn mark_spawned(&mut self, now: Instant) {
        self.last_spawn = Some(now);
        self.turns_since_spawn = 0;
    }

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

fn build_nudge_spawn() -> SpawnRequestEvent {
    spawn_event(
        "learning:nudge",
        "Review the recent conversation in recent_messages.json for anything worth learning — \
         user preferences, corrections, recovery from errors — corroborate against memory, and \
         persist per your instructions."
            .to_string(),
    )
}

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
        model: residuum_config::ModelSelector::LARGE,
        spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
        depth: MAIN_DEPTH + 1,
        hop_count: 0,
        sender: None,
        conversation: None,
        inbound: None,
        images: Vec::new(),
        overlap: None,
        carried_history: Vec::new(),
        fire_id: None,
        delivered_messages: Vec::new(),
        delivered_chat_messages: Vec::new(),
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

        let spawn = state.on_learn_signals(&signals, COOLDOWN, now).unwrap();
        assert_eq!(spawn.skill.as_ref().map(AsRef::as_ref), Some("learner"));
        assert_eq!(spawn.source_label, "learning:subconscious");
        assert!(spawn.prompt.contains("User prefers bullets."));
        assert!(spawn.prompt.contains("Worked around a 502."));
        assert!(spawn.prompt.contains("recent_messages.json"));
        assert!(matches!(spawn.model, residuum_config::ModelSelector::LARGE));
        assert!(spawn.address.as_ref().starts_with("spawned-learner-"));

        assert!(
            state.on_learn_signals(&signals, COOLDOWN, now).is_none(),
            "cooldown should suppress the second spawn"
        );

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

        assert!(state.on_turn_completed(3, COOLDOWN, now).is_none());
        assert!(state.on_turn_completed(3, COOLDOWN, now).is_none());
        let spawn = state.on_turn_completed(3, COOLDOWN, now).unwrap();
        assert_eq!(spawn.source_label, "learning:nudge");
        assert!(spawn.prompt.contains("recent_messages.json"));

        let later = now.checked_add(COOLDOWN).unwrap();
        assert!(state.on_turn_completed(3, COOLDOWN, later).is_none());
        assert!(state.on_turn_completed(3, COOLDOWN, later).is_none());
        assert!(state.on_turn_completed(3, COOLDOWN, later).is_some());
    }

    #[test]
    fn nudge_respects_cooldown_and_retries() {
        let mut state = LearningState::default();
        let now = Instant::now();

        state
            .on_learn_signals(&[signal("x", LearnSignalType::Preference)], COOLDOWN, now)
            .unwrap();

        assert!(state.on_turn_completed(1, COOLDOWN, now).is_none());
        assert!(state.on_turn_completed(1, COOLDOWN, now).is_none());

        let later = now.checked_add(COOLDOWN).unwrap();
        assert!(state.on_turn_completed(1, COOLDOWN, later).is_some());
    }

    #[test]
    fn signals_suppressed_by_cooldown_are_queued_into_next_spawn() {
        let mut state = LearningState::default();
        let now = Instant::now();

        state
            .on_learn_signals(
                &[signal("first", LearnSignalType::Preference)],
                COOLDOWN,
                now,
            )
            .unwrap();

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

        state
            .on_learn_signals(
                &[signal("armed", LearnSignalType::Preference)],
                COOLDOWN,
                now,
            )
            .unwrap();

        assert!(
            state
                .on_learn_signals(
                    &[signal("queued-for-nudge", LearnSignalType::Recovery)],
                    COOLDOWN,
                    now
                )
                .is_none()
        );

        let later = now.checked_add(COOLDOWN).unwrap();
        let spawn = state.on_turn_completed(1, COOLDOWN, later).unwrap();
        assert_eq!(
            spawn.source_label, "learning:subconscious",
            "a nudge with queued signals should use the signal-batch spawn, not the generic nudge"
        );
        assert!(spawn.prompt.contains("queued-for-nudge"));
    }
}
