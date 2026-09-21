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
    /// Concise single-sentence observations extracted from the conversation.
    pub(crate) observations: Vec<String>,
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

/// Which kind of document a memory search result came from.
///
/// The memory index holds three kinds of documents: distilled [`Observation`]s,
/// raw interaction-pair [`IndexChunk`]s, and knowledge wiki pages. `DocSource`
/// is the single internal vocabulary for that distinction — it is the value
/// stored in the index's `source_type` field, the value filtered on, and the
/// value shown in results. The `memory_search` tool maps its user-facing names
/// (`"observations"`/`"episodes"`/`"wiki"`) onto these variants at its boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocSource {
    /// A distilled observation document.
    Observation,
    /// A raw interaction-pair chunk document.
    Chunk,
    /// A knowledge wiki page, one document per page.
    Wiki,
}

impl DocSource {
    /// The stable string stored in the index and shown in search results.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Chunk => "chunk",
            Self::Wiki => "wiki",
        }
    }

    /// Parse the value stored in the index's `source_type` field.
    ///
    /// Returns `None` for any value the index should never contain — a signal
    /// of index corruption rather than a normal input.
    #[must_use]
    pub fn from_index_value(value: &str) -> Option<Self> {
        match value {
            "observation" => Some(Self::Observation),
            "chunk" => Some(Self::Chunk),
            "wiki" => Some(Self::Wiki),
            _ => None,
        }
    }
}

impl fmt::Display for DocSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Accepts both the current plain-string form and the legacy on-disk array form
/// (`["ep-001"]` or `[]`) for `Observation::source_episodes`, normalizing either to
/// `Option<String>`. The field never holds more than one ID; a legacy array with
/// extras keeps the first and drops the rest.
fn deserialize_source_episode<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SourceEpisode {
        One(String),
        Many(Vec<String>),
    }

    Ok(match Option::<SourceEpisode>::deserialize(deserializer)? {
        None => None,
        Some(SourceEpisode::One(id)) => Some(id),
        Some(SourceEpisode::Many(mut ids)) => {
            if ids.is_empty() {
                None
            } else {
                Some(ids.remove(0))
            }
        }
    })
}

/// Where a merged observation or episode came from.
///
/// Unset (all `None`) for the main agent's own observations. Set when a
/// session's run was merged into global memory, so the source stays
/// traceable through search and `memory_get`. All fields are optional on
/// read so records written before session memory existed still load.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceTag {
    /// The session's address, when this record came from a session run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_address: Option<String>,
    /// The run id within that session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The session's category (`"scheduled"`, `"external"`, or `"spawned"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

impl SourceTag {
    /// No source tag: the main agent's own observation or episode.
    #[must_use]
    pub fn main() -> Self {
        Self::default()
    }

    /// Tag identifying the session run a merged observation or episode came from.
    #[must_use]
    pub fn session(
        address: impl Into<String>,
        run_id: impl Into<String>,
        category: impl Into<String>,
    ) -> Self {
        Self {
            session_address: Some(address.into()),
            run_id: Some(run_id.into()),
            category: Some(category.into()),
        }
    }

    /// Whether this tag identifies a session run (as opposed to the main agent).
    #[must_use]
    pub fn is_session(&self) -> bool {
        self.session_address.is_some()
    }
}

/// A single extracted observation with full metadata.
///
/// Each observation is self-describing: it carries when it was created,
/// which episode transcript it came from, and whether it originated from
/// a user-visible or background turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// When this observation was created.
    #[serde(with = "crate::time::minute_format")]
    pub timestamp: NaiveDateTime,
    /// ID of the episode transcript that produced this observation.
    #[serde(
        default,
        deserialize_with = "deserialize_source_episode",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_episodes: Option<String>,
    /// Whether this observation came from a user-visible or background turn.
    pub visibility: Visibility,
    /// The observation content as a single concise sentence.
    pub content: String,
    /// Session/run/category this observation was merged from, if any.
    #[serde(flatten)]
    pub source: SourceTag,
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

        if let Some(source_episode) = &self.source_episodes {
            write!(f, " | [{source_episode}]")?;
        }

        write!(f, " | {}\n  {}", self.visibility, self.content)
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
        lines.push("Format: [timestamp] | [source episode] | [visibility]".to_string());
        for obs in &self.observations {
            lines.push(obs.to_string());
        }
        lines.join("\n")
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
        Self::default()
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

        crate::util::fs::atomic_write(path, &json).await
    }
}

#[cfg(test)]
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
            source_episodes: Some("ep-001".to_string()),
            visibility: Visibility::User,
            content: "tantivy provides BM25 search without C dependencies".to_string(),
            source: SourceTag::main(),
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
            deserialized.source_episodes,
            Some("ep-001".to_string()),
            "source_episodes should round-trip"
        );
        assert_eq!(
            deserialized.visibility,
            Visibility::User,
            "visibility should round-trip"
        );
    }

    #[test]
    fn observation_deserializes_legacy_single_element_array() {
        let json = r#"{
            "timestamp": "2024-02-19T00:00",
            "source_episodes": ["ep-001"],
            "visibility": "user",
            "content": "legacy on-disk format"
        }"#;
        let obs: Observation = serde_json::from_str(json).unwrap();
        assert_eq!(
            obs.source_episodes,
            Some("ep-001".to_string()),
            "a legacy one-element array should deserialize to Some"
        );
    }

    #[test]
    fn observation_deserializes_legacy_empty_array() {
        let json = r#"{
            "timestamp": "2024-02-19T00:00",
            "source_episodes": [],
            "visibility": "user",
            "content": "legacy on-disk format"
        }"#;
        let obs: Observation = serde_json::from_str(json).unwrap();
        assert_eq!(
            obs.source_episodes, None,
            "a legacy empty array should deserialize to None"
        );
    }

    #[test]
    fn observation_deserializes_missing_field_as_none() {
        let json = r#"{
            "timestamp": "2024-02-19T00:00",
            "visibility": "user",
            "content": "no source_episodes key at all"
        }"#;
        let obs: Observation = serde_json::from_str(json).unwrap();
        assert_eq!(
            obs.source_episodes, None,
            "a missing field should default to None"
        );
    }

    #[test]
    fn observation_source_episodes_skipped_when_none() {
        let obs = Observation {
            source_episodes: None,
            ..sample_observation()
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(
            !json.contains("source_episodes"),
            "absent source_episodes should be skipped in serialization"
        );
    }

    #[test]
    fn doc_source_as_str_round_trips_through_index_value() {
        for source in [DocSource::Observation, DocSource::Chunk, DocSource::Wiki] {
            assert_eq!(
                DocSource::from_index_value(source.as_str()),
                Some(source),
                "as_str should round-trip through from_index_value"
            );
        }
    }

    #[test]
    fn doc_source_index_values_are_stable() {
        assert_eq!(DocSource::Observation.as_str(), "observation");
        assert_eq!(DocSource::Chunk.as_str(), "chunk");
        assert_eq!(DocSource::Wiki.as_str(), "wiki");
    }

    #[test]
    fn doc_source_display_matches_index_value() {
        assert_eq!(DocSource::Observation.to_string(), "observation");
        assert_eq!(DocSource::Chunk.to_string(), "chunk");
    }

    #[test]
    fn doc_source_rejects_unknown_index_value() {
        assert_eq!(DocSource::from_index_value("episodes"), None);
        assert_eq!(DocSource::from_index_value(""), None);
        assert_eq!(DocSource::from_index_value("Observation"), None);
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
        log.observations.push(sample_observation());

        let json = serde_json::to_string(&log).unwrap();
        let deserialized: ObservationLog = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.observations.len(),
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
            "[2024-02-19T00:00] | [ep-001] | user\n  tantivy provides BM25 search without C dependencies"
        );
    }

    #[test]
    fn observation_display_without_sources() {
        let obs = Observation {
            source_episodes: None,
            ..sample_observation()
        };
        let formatted = obs.to_string();
        assert_eq!(
            formatted,
            "[2024-02-19T00:00] | user\n  tantivy provides BM25 search without C dependencies"
        );
    }

    #[test]
    fn display_formatted_includes_key_line() {
        let mut log = ObservationLog::new();
        log.observations.push(sample_observation());
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
        assert!(log.observations.is_empty(), "new log should be empty");
        assert_eq!(log.observations.len(), 0, "new log should have length 0");
    }

    #[test]
    fn observation_log_push() {
        let mut log = ObservationLog::new();
        log.observations.push(sample_observation());
        assert!(
            !log.observations.is_empty(),
            "log should not be empty after push"
        );
        assert_eq!(
            log.observations.len(),
            1,
            "log should have one observation after push"
        );
    }

    #[test]
    fn index_chunk_serde_round_trip() {
        let chunk = IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
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
