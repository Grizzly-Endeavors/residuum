//! Core memory data types for episodes and observation logs.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// A compressed episode extracted from a conversation segment.
///
/// Episodes capture the key decisions, problems, solutions, and context
/// from a block of conversation messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    /// Unique identifier (e.g., `"ep-001"` or `"ref-001"` for reflected).
    pub id: String,
    /// Date of the episode.
    pub date: NaiveDate,
    /// One-line summary of how the episode started.
    pub start: String,
    /// One-line summary of how the episode ended.
    pub end: String,
    /// The project or topic context tag.
    pub context: String,
    /// Concise single-sentence observations extracted from the conversation.
    pub observations: Vec<String>,
    /// IDs of episodes that were merged to create this one (for reflected episodes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_episodes: Vec<String>,
}

/// Transparent wrapper over a list of episodes for serialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObservationLog {
    /// The episodes in the observation log.
    pub episodes: Vec<Episode>,
}

impl ObservationLog {
    /// Create an empty observation log.
    #[must_use]
    pub fn new() -> Self {
        Self {
            episodes: Vec::new(),
        }
    }

    /// Add an episode to the log.
    pub fn push(&mut self, episode: Episode) {
        self.episodes.push(episode);
    }

    /// Get the number of episodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.episodes.len()
    }

    /// Check if the log is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    fn sample_episode() -> Episode {
        Episode {
            id: "ep-001".to_string(),
            date: NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            start: "user asked about memory design".to_string(),
            end: "settled on episode-based approach".to_string(),
            context: "ironclaw/memory".to_string(),
            observations: vec![
                "episodes compress conversation segments into structured data".to_string(),
                "tantivy provides BM25 search without C dependencies".to_string(),
            ],
            source_episodes: vec![],
        }
    }

    #[test]
    fn episode_serde_round_trip() {
        let episode = sample_episode();
        let json = serde_json::to_string(&episode).unwrap();
        let deserialized: Episode = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "ep-001", "id should round-trip");
        assert_eq!(
            deserialized.date,
            NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            "date should round-trip"
        );
        assert_eq!(
            deserialized.observations.len(),
            2,
            "observations should round-trip"
        );
        assert!(
            deserialized.source_episodes.is_empty(),
            "empty source_episodes should round-trip"
        );
    }

    #[test]
    fn episode_source_episodes_skipped_when_empty() {
        let episode = sample_episode();
        let json = serde_json::to_string(&episode).unwrap();
        assert!(
            !json.contains("source_episodes"),
            "empty source_episodes should be skipped in serialization"
        );
    }

    #[test]
    fn episode_source_episodes_preserved_when_set() {
        let episode = Episode {
            id: "ref-001".to_string(),
            source_episodes: vec!["ep-001".to_string(), "ep-002".to_string()],
            ..sample_episode()
        };
        let json = serde_json::to_string(&episode).unwrap();
        let deserialized: Episode = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.source_episodes.len(),
            2,
            "source_episodes should be preserved"
        );
    }

    #[test]
    fn observation_log_serde_round_trip() {
        let mut log = ObservationLog::new();
        log.push(sample_episode());

        let json = serde_json::to_string(&log).unwrap();
        let deserialized: ObservationLog = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.len(),
            1,
            "log should round-trip with one episode"
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
        log.push(sample_episode());
        assert!(!log.is_empty(), "log should not be empty after push");
        assert_eq!(log.len(), 1, "log should have one episode after push");
    }

    #[test]
    fn episode_deserialize_without_source_episodes() {
        let json = r#"{
            "id": "ep-001",
            "date": "2026-02-19",
            "start": "started",
            "end": "ended",
            "context": "test",
            "observations": ["one"]
        }"#;
        let episode: Episode = serde_json::from_str(json).unwrap();
        assert!(
            episode.source_episodes.is_empty(),
            "missing source_episodes should default to empty"
        );
    }
}
