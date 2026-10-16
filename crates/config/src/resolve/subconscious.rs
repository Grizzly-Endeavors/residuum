//! Subconscious classifier and learning-loop settings.

use super::super::deserialize::{LearningConfigFile, SubconsciousConfigFile};
use super::super::types::{LearningConfig, SubconsciousSettings};

/// Resolve subconscious settings from TOML section (opt-in, default disabled).
pub(super) fn resolve_subconscious_settings(
    section: Option<&SubconsciousConfigFile>,
) -> SubconsciousSettings {
    let mut settings = SubconsciousSettings::default();
    if let Some(s) = section {
        if let Some(v) = s.enabled {
            settings.enabled = v;
        }
        if let Some(v) = s.mid_turn {
            settings.mid_turn = v;
        }
        if let Some(v) = s.every_n_iterations {
            settings.every_n_iterations = v;
        }
        if let Some(v) = s.max_transcript_tokens {
            settings.max_transcript_tokens = v;
        }
        if let Some(v) = s.learning {
            settings.learning = v;
        }
        if let Some(v) = s.learning_cooldown_minutes {
            settings.learning_cooldown_minutes = v;
        }
    }
    settings
}

/// Resolve the activity-triggered learning fallback config (turn-count nudge).
pub(super) fn resolve_learning_config(section: Option<&LearningConfigFile>) -> LearningConfig {
    let mut cfg = LearningConfig::default();
    if let Some(s) = section
        && let Some(v) = s.nudge_after_turns
    {
        cfg.nudge_after_turns = v;
    }
    cfg
}
