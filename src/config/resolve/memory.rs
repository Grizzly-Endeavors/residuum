//! Memory thresholds and search weights.

use super::super::deserialize::{MemoryConfigFile, SearchConfigFile};
use super::super::types::{MemoryConfig, SearchConfig};

/// Resolve memory subsystem configuration from TOML section with defaults.
pub(super) fn resolve_memory_config(section: Option<&MemoryConfigFile>) -> MemoryConfig {
    let mut mem = MemoryConfig::default();
    if let Some(s) = section {
        if let Some(v) = s.observer_threshold_tokens {
            mem.observer_threshold_tokens = v;
        }
        if let Some(v) = s.reflector_threshold_tokens {
            mem.reflector_threshold_tokens = v;
        }
        if let Some(v) = s.observer_cooldown_secs {
            mem.observer_cooldown_secs = v;
        }
        if let Some(v) = s.observer_force_threshold_tokens {
            mem.observer_force_threshold_tokens = v;
        }
    }
    mem.search = resolve_search_config(section.and_then(|m| m.search.as_ref()));
    mem
}

/// Resolve hybrid search configuration from TOML section with defaults.
pub(super) fn resolve_search_config(section: Option<&SearchConfigFile>) -> SearchConfig {
    let mut cfg = SearchConfig::default();

    if let Some(s) = section {
        if let Some(v) = s.vector_weight {
            cfg.vector_weight = valid_search_weight("vector_weight", v, cfg.vector_weight);
        }
        if let Some(v) = s.text_weight {
            cfg.text_weight = valid_search_weight("text_weight", v, cfg.text_weight);
        }
        if let Some(v) = s.min_score {
            cfg.min_score = v;
        }
        if let Some(v) = s.candidate_multiplier {
            if v == 0 {
                tracing::warn!(
                    section = "memory.search",
                    value = v,
                    default = cfg.candidate_multiplier,
                    "candidate_multiplier must be positive; using default"
                );
            } else {
                cfg.candidate_multiplier = v;
            }
        }
        if let Some(v) = s.temporal_decay {
            cfg.temporal_decay = v;
        }
        if let Some(v) = s.temporal_decay_half_life_days {
            if v <= 0.0 {
                tracing::warn!(
                    section = "memory.search",
                    value = v,
                    default = cfg.temporal_decay_half_life_days,
                    "temporal_decay_half_life_days must be positive; using default"
                );
            } else {
                cfg.temporal_decay_half_life_days = v;
            }
        }
    }

    // Normalized so the merged score stays in [0, 1], the range `min_score`
    // is expressed in. Only the ratio between the two weights is meaningful.
    let sum = cfg.vector_weight + cfg.text_weight;
    if sum > 0.0 {
        cfg.vector_weight /= sum;
        cfg.text_weight /= sum;
    } else {
        let defaults = SearchConfig::default();
        tracing::warn!(
            section = "memory.search",
            default_vector_weight = defaults.vector_weight,
            default_text_weight = defaults.text_weight,
            "vector_weight and text_weight are both zero; using defaults"
        );
        cfg.vector_weight = defaults.vector_weight;
        cfg.text_weight = defaults.text_weight;
    }

    cfg
}

/// A hybrid search weight from config, or `fallback` when it is negative or
/// not a finite number.
fn valid_search_weight(key: &str, value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        tracing::warn!(
            section = "memory.search",
            key,
            value,
            default = fallback,
            "search weight must be a non-negative number; using default"
        );
        fallback
    }
}
