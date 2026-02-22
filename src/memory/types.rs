//! Core memory data types for observations and the observation log.

use std::fmt;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// A compressed episode extracted from a conversation segment.
///
/// Used internally for LLM parsing (observer and reflector responses).
/// Not exposed publicly — callers work with [`Observation`] instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Episode {
    /// Unique identifier (e.g., `"ep-001"`).
    pub(crate) id: String,
    /// Date of the episode.
    pub(crate) date: chrono::NaiveDate,
    /// One-line summary of how the episode started.
    pub(crate) start: String,
    /// One-line summary of how the episode ended.
    pub(crate) end: String,
    /// The project or topic context tag.
    pub(crate) context: String,
    /// Concise single-sentence observations extracted from the conversation.
    pub(crate) observations: Vec<String>,
    /// IDs of episodes that were merged to create this one (for reflected episodes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) source_episodes: Vec<String>,
}

/// Visibility of an observation relative to the conversation context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Observation came from a user-visible conversation turn.
    #[default]
    User,
    /// Observation came from a background system turn (pulse, cron).
    Background,
}

/// A single extracted observation with full metadata.
///
/// Each observation is self-describing: it carries when it was created,
/// what project it belongs to, which episode transcript(s) it came from,
/// and whether it originated from a user-visible or background turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// When this observation was created.
    #[serde(with = "crate::time::minute_format")]
    pub timestamp: NaiveDateTime,
    /// Project or workspace context at the time of observation.
    pub project_context: String,
    /// IDs of the episode transcript files that produced this observation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_episodes: Vec<String>,
    /// Whether this observation came from a user-visible or background turn.
    pub visibility: Visibility,
    /// The observation content as a single concise sentence.
    pub content: String,
}

/// Flat list of all observations across sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObservationLog {
    /// All observations in chronological order.
    pub observations: Vec<Observation>,
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => f.write_str("user"),
            Self::Background => f.write_str("background"),
        }
    }
}

impl fmt::Display for Observation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.timestamp.format("%Y-%m-%dT%H:%M"))?;

        if !self.source_episodes.is_empty() {
            write!(f, " | [{}]", self.source_episodes.join(", "))?;
        }

        write!(
            f,
            " | {} | {}\n  {}",
            self.project_context, self.visibility, self.content
        )
    }
}

impl ObservationLog {
    /// Create an empty observation log.
    #[must_use]
    pub fn new() -> Self {
        Self {
            observations: Vec::new(),
        }
    }

    /// Format all observations as human-readable text for the system prompt.
    ///
    /// Produces a key line followed by one entry per observation.
    #[must_use]
    pub fn display_formatted(&self) -> String {
        if self.observations.is_empty() {
            return String::new();
        }

        let mut lines = Vec::with_capacity(self.observations.len() + 1);
        lines
            .push("Format: [timestamp] | [source episodes] | [project] | [visibility]".to_string());
        for obs in &self.observations {
            lines.push(obs.to_string());
        }
        lines.join("\n")
    }

    /// Add an observation to the log.
    pub fn push(&mut self, observation: Observation) {
        self.observations.push(observation);
    }

    /// Get the number of observations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.observations.len()
    }

    /// Check if the log is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    fn sample_observation() -> Observation {
        Observation {
            timestamp: chrono::NaiveDate::from_ymd_opt(2024, 2, 19)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            project_context: "ironclaw/memory".to_string(),
            source_episodes: vec!["ep-001".to_string()],
            visibility: Visibility::User,
            content: "tantivy provides BM25 search without C dependencies".to_string(),
        }
    }

    #[test]
    fn observation_serde_round_trip() {
        let obs = sample_observation();
        let json = serde_json::to_string(&obs).unwrap();
        let deserialized: Observation = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.content, obs.content,
            "content should round-trip"
        );
        assert_eq!(
            deserialized.project_context, "ironclaw/memory",
            "project_context should round-trip"
        );
        assert_eq!(
            deserialized.source_episodes,
            vec!["ep-001"],
            "source_episodes should round-trip"
        );
        assert_eq!(
            deserialized.visibility,
            Visibility::User,
            "visibility should round-trip"
        );
    }

    #[test]
    fn observation_source_episodes_skipped_when_empty() {
        let obs = Observation {
            source_episodes: vec![],
            ..sample_observation()
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(
            !json.contains("source_episodes"),
            "empty source_episodes should be skipped in serialization"
        );
    }

    #[test]
    fn visibility_default_is_user() {
        let vis = Visibility::default();
        assert_eq!(vis, Visibility::User, "default visibility should be User");
    }

    #[test]
    fn visibility_serde_snake_case() {
        let user = serde_json::to_string(&Visibility::User).unwrap();
        let bg = serde_json::to_string(&Visibility::Background).unwrap();
        assert_eq!(user, r#""user""#, "User should serialize as snake_case");
        assert_eq!(
            bg, r#""background""#,
            "Background should serialize as snake_case"
        );
    }

    #[test]
    fn observation_log_serde_round_trip() {
        let mut log = ObservationLog::new();
        log.push(sample_observation());

        let json = serde_json::to_string(&log).unwrap();
        let deserialized: ObservationLog = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.len(),
            1,
            "log should round-trip with one observation"
        );
    }

    #[test]
    fn observation_display_with_sources() {
        let obs = sample_observation();
        let formatted = obs.to_string();
        assert_eq!(
            formatted,
            "[2024-02-19T00:00] | [ep-001] | ironclaw/memory | user\n  tantivy provides BM25 search without C dependencies"
        );
    }

    #[test]
    fn observation_display_without_sources() {
        let obs = Observation {
            source_episodes: vec![],
            ..sample_observation()
        };
        let formatted = obs.to_string();
        assert_eq!(
            formatted,
            "[2024-02-19T00:00] | ironclaw/memory | user\n  tantivy provides BM25 search without C dependencies"
        );
    }

    #[test]
    fn display_formatted_includes_key_line() {
        let mut log = ObservationLog::new();
        log.push(sample_observation());
        let formatted = log.display_formatted();
        assert!(
            formatted.starts_with("Format: [timestamp]"),
            "should start with key line"
        );
        assert!(
            formatted.contains("tantivy provides BM25"),
            "should include observation content"
        );
    }

    #[test]
    fn display_formatted_empty_log() {
        let log = ObservationLog::new();
        assert!(
            log.display_formatted().is_empty(),
            "empty log should produce empty string"
        );
    }

    #[test]
    fn observation_log_empty() {
        let log = ObservationLog::new();
        assert!(log.is_empty(), "new log should be empty");
        assert_eq!(log.len(), 0, "new log should have length 0");
    }

    #[test]
    fn observation_log_push() {
        let mut log = ObservationLog::new();
        log.push(sample_observation());
        assert!(!log.is_empty(), "log should not be empty after push");
        assert_eq!(log.len(), 1, "log should have one observation after push");
    }
}
