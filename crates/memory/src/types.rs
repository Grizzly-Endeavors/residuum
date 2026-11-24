use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use anyhow::Context;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: String,
    pub date: chrono::NaiveDate,
    pub observations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Visibility {
    #[default]
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocSource {
    Observation,
    Chunk,
    Wiki,
}

impl DocSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Chunk => "chunk",
            Self::Wiki => "wiki",
        }
    }

    #[must_use]
    pub(crate) fn from_index_value(value: &str) -> Option<Self> {
        match value {
            "observation" => Some(Self::Observation),
            "chunk" => Some(Self::Chunk),
            "wiki" => Some(Self::Wiki),
            _ => None,
        }
    }

    #[must_use]
    pub fn from_query_str(value: &str) -> Option<Self> {
        match value {
            "observations" => Some(Self::Observation),
            "episodes" => Some(Self::Chunk),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceTag {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

impl SourceTag {
    #[must_use]
    pub fn main() -> Self {
        Self::default()
    }

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

    #[must_use]
    pub fn is_session(&self) -> bool {
        self.session_address.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    #[serde(with = "residuum_core::time::minute_format")]
    pub timestamp: NaiveDateTime,
    #[serde(
        default,
        deserialize_with = "deserialize_source_episode",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_episodes: Option<String>,
    pub visibility: Visibility,
    pub content: String,
    #[serde(flatten)]
    pub source: SourceTag,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObservationLog {
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
    #[must_use]
    pub fn new() -> Self {
        Self {
            observations: Vec::new(),
        }
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexChunk {
    pub chunk_id: String,
    pub episode_id: String,
    pub date: String,
    pub line_start: usize,
    pub line_end: usize,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFileEntry {
    pub(crate) mtime: String,
    pub doc_ids: Vec<String>,
    #[serde(default)]
    pub embedded: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexManifest {
    pub last_rebuild: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_dim: Option<usize>,
    pub files: HashMap<String, ManifestFileEntry>,
}

impl IndexManifest {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

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

    pub async fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json =
            serde_json::to_string_pretty(self).context("failed to serialize index manifest")?;

        residuum_core::util::fs::atomic_write(path, &json).await
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
    fn observation_without_source_tag_loads_as_main() {
        let json = r#"{
            "timestamp": "2024-02-19T00:00",
            "source_episodes": "ep-001",
            "visibility": "user",
            "content": "written before session memory existed"
        }"#;
        let obs: Observation = serde_json::from_str(json).unwrap();
        assert!(
            !obs.source.is_session(),
            "an observation with no source tag fields should load as untagged"
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
    fn doc_source_from_query_str_maps_user_facing_names() {
        assert_eq!(
            DocSource::from_query_str("observations"),
            Some(DocSource::Observation)
        );
        assert_eq!(
            DocSource::from_query_str("episodes"),
            Some(DocSource::Chunk)
        );
        assert_eq!(DocSource::from_query_str("wiki"), Some(DocSource::Wiki));
    }

    #[test]
    fn doc_source_from_query_str_rejects_unknown_name() {
        assert_eq!(DocSource::from_query_str("observation"), None);
        assert_eq!(DocSource::from_query_str(""), None);
        assert_eq!(DocSource::from_query_str("Wiki"), None);
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
