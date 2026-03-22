//! Core memory data types for observations and the observation log.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use anyhow::Context;

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
    /// Observation came from a background system turn (pulse, actions).
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

/// A single chunk from an episode's idx.jsonl file — one interaction pair or other segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexChunk {
    /// Unique chunk identifier (e.g., `"ep-001-c0"`).
    pub chunk_id: String,
    /// Parent episode identifier.
    pub episode_id: String,
    /// Date string in `YYYY-MM-DD` format.
    pub date: String,
    /// Project context tag.
    pub context: String,
    /// Line number of the first message in this chunk (in the transcript).
    pub line_start: usize,
    /// Line number of the last message in this chunk (in the transcript).
    pub line_end: usize,
    /// Searchable text content (user question + assistant text response).
    pub content: String,
}

/// File entry in the index manifest tracking what has been indexed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFileEntry {
    /// File modification time as ISO string.
    pub mtime: String,
    /// Document IDs that were indexed from this file.
    pub doc_ids: Vec<String>,
    /// Whether this file's observations/chunks have been embedded in the vector store.
    #[serde(default)]
    pub embedded: bool,
}

/// Manifest tracking which files have been indexed and their state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexManifest {
    /// Timestamp of the last full rebuild.
    pub last_rebuild: String,
    /// Embedding model name (for future vector search).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// Embedding dimension (for future vector search).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_dim: Option<usize>,
    /// Map of relative file path to its indexed state.
    pub files: HashMap<String, ManifestFileEntry>,
}

impl IndexManifest {
    /// Create a new empty manifest.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_rebuild: String::new(),
            embedding_model: None,
            embedding_dim: None,
            files: HashMap::new(),
        }
    }

    /// Load the manifest from disk. Returns an empty manifest if the file is missing.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        match tokio::fs::read_to_string(path).await {
            Ok(contents) => serde_json::from_str(&contents)
                .with_context(|| format!("failed to parse index manifest at {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(anyhow::Error::new(e).context(format!(
                "failed to read index manifest at {}",
                path.display()
            ))),
        }
    }

    /// Save the manifest to disk atomically (temp file + rename).
    ///
    /// # Errors
    /// Returns an error if the file cannot be written.
    pub async fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json =
            serde_json::to_string_pretty(self).context("failed to serialize index manifest")?;

        crate::fs::atomic_write(path, &json).await
    }
}

impl Default for IndexManifest {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes into known-length collections"
)]
mod tests {
    use super::*;

    fn sample_observation() -> Observation {
        Observation {
            timestamp: chrono::NaiveDate::from_ymd_opt(2024, 2, 19)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            project_context: "residuum/memory".to_string(),
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
            deserialized.project_context, "residuum/memory",
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
            "[2024-02-19T00:00] | [ep-001] | residuum/memory | user\n  tantivy provides BM25 search without C dependencies"
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
            "[2024-02-19T00:00] | residuum/memory | user\n  tantivy provides BM25 search without C dependencies"
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

    #[test]
    fn index_chunk_serde_round_trip() {
        let chunk = IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 2,
            line_end: 3,
            content: "user: hello\nassistant: hi there".to_string(),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        let deserialized: IndexChunk = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.chunk_id, "ep-001-c0");
        assert_eq!(deserialized.line_start, 2);
        assert_eq!(deserialized.content, chunk.content);
    }

    #[test]
    fn index_manifest_new_is_empty() {
        let manifest = IndexManifest::new();
        assert!(
            manifest.files.is_empty(),
            "new manifest should have no files"
        );
        assert!(
            manifest.last_rebuild.is_empty(),
            "new manifest should have empty last_rebuild"
        );
        assert!(manifest.embedding_model.is_none());
        assert!(manifest.embedding_dim.is_none());
    }

    #[test]
    fn index_manifest_serde_round_trip() {
        let mut manifest = IndexManifest::new();
        manifest.last_rebuild = "2026-02-19T14:00".to_string();
        manifest.files.insert(
            "episodes/2026-02/19/ep-001.obs.json".to_string(),
            ManifestFileEntry {
                mtime: "2026-02-19T14:30:00".to_string(),
                doc_ids: vec!["ep-001-o0".to_string(), "ep-001-o1".to_string()],
                embedded: false,
            },
        );
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let deserialized: IndexManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.last_rebuild, "2026-02-19T14:00");
        assert_eq!(deserialized.files.len(), 1);
        assert_eq!(
            deserialized.files["episodes/2026-02/19/ep-001.obs.json"]
                .doc_ids
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn index_manifest_load_missing_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let manifest = IndexManifest::load(&path).await.unwrap();
        assert!(manifest.files.is_empty());
    }

    #[tokio::test]
    async fn index_manifest_save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");

        let mut manifest = IndexManifest::new();
        manifest.last_rebuild = "2026-02-19T14:00".to_string();
        manifest.files.insert(
            "test.obs.json".to_string(),
            ManifestFileEntry {
                mtime: "2026-02-19T14:30:00".to_string(),
                doc_ids: vec!["id-1".to_string()],
                embedded: false,
            },
        );

        manifest.save(&path).await.unwrap();
        let loaded = IndexManifest::load(&path).await.unwrap();
        assert_eq!(loaded.last_rebuild, "2026-02-19T14:00");
        assert_eq!(loaded.files.len(), 1);
    }

    #[test]
    fn manifest_file_entry_serde() {
        let entry = ManifestFileEntry {
            mtime: "2026-02-19T14:30:00".to_string(),
            doc_ids: vec!["a".to_string(), "b".to_string()],
            embedded: true,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: ManifestFileEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.mtime, "2026-02-19T14:30:00");
        assert_eq!(deserialized.doc_ids, vec!["a", "b"]);
        assert!(deserialized.embedded, "embedded should round-trip");
    }

    #[test]
    fn manifest_file_entry_embedded_defaults_false() {
        let json = r#"{"mtime":"2026-02-19T14:30:00","doc_ids":["a"]}"#;
        let entry: ManifestFileEntry = serde_json::from_str(json).unwrap();
        assert!(!entry.embedded, "embedded should default to false");
    }
}
