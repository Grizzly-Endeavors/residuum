//! `config/a2a.json`: the remote agents this instance's A2A client can
//! reach, keyed by name. Loader modeled on
//! `crate::workspace::config::load_mcp_servers_map`: a bad entry is skipped
//! with a warning rather than failing the whole file. See
//! `docs/systems-usage/a2a.md`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context as _;
use serde::Deserialize;

use crate::agent_keys::AgentKeyStore;

/// Longest accepted agent name.
const MAX_NAME_LEN: usize = 64;

/// Check an agent name: `[a-z][a-z0-9_]*`, at most 64 characters — the same
/// shape as an agent-key name (`crate::agent_keys::validate_name`), reused
/// here because `a2a:<name>` addresses live alongside other agent
/// identifiers the model sees.
#[must_use]
pub fn is_valid_agent_name(name: &str) -> bool {
    name.len() <= MAX_NAME_LEN
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// One remote agent entry after header expansion, ready to use for a card
/// fetch or a protocol client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A2aAgentEntry {
    pub name: String,
    pub url: String,
    pub headers: HashMap<String, String>,
}

/// Raw JSON structure for `config/a2a.json`.
#[derive(Deserialize)]
struct A2aAgentsFile {
    #[serde(default)]
    agents: HashMap<String, A2aAgentRaw>,
}

#[derive(Deserialize)]
struct A2aAgentRaw {
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

/// Validate `content` as a well-formed `config/a2a.json`: valid JSON matching
/// the `{"agents": {"<name>": {"url": ..., "headers"?: ...}}}` shape, with
/// every agent name well-formed and every url non-empty. Does not expand
/// `${agent-key:...}`/`${ENV}` references or check reachability — those are
/// handled (skip-with-warning, or a background retry) once the file actually
/// loads, not at this validation step. Used by the web settings API before
/// writing a raw edit.
///
/// # Errors
/// Returns a plain-language message describing the first problem found.
pub fn validate_a2a_agents_json(content: &str) -> Result<(), String> {
    let file: A2aAgentsFile =
        serde_json::from_str(content).map_err(|e| format!("invalid JSON: {e}"))?;
    for (name, raw) in &file.agents {
        if !is_valid_agent_name(name) {
            return Err(format!(
                "agent name '{name}' must start with a lowercase letter and contain only \
                 lowercase letters, digits, and underscores (at most {MAX_NAME_LEN} characters)"
            ));
        }
        if raw.url.trim().is_empty() {
            return Err(format!("agent '{name}' must have a non-empty url"));
        }
    }
    Ok(())
}

/// Load `config/a2a.json` as a name → entry map, expanding
/// `${agent-key:<name>}` (via `crate::agent_keys::expand_references`) and
/// `${ENV}` (via `crate::mcp::client::expand_env_vars`, the same helper MCP
/// server headers use) references in header values.
///
/// An entry with an invalid name, an empty url, or a header referencing an
/// unknown agent key is skipped with a warning; the rest of the file still
/// loads.
///
/// Returns an empty map if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed as JSON.
#[tracing::instrument(skip_all, fields(path = %path.display()))]
pub fn load_a2a_agents_map(
    path: &Path,
    agent_keys: &AgentKeyStore,
) -> anyhow::Result<HashMap<String, A2aAgentEntry>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read a2a.json at {}", path.display()))?;
    let file: A2aAgentsFile = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse a2a.json at {}", path.display()))?;

    let agents: HashMap<String, A2aAgentEntry> = file
        .agents
        .into_iter()
        .filter_map(|(name, raw)| {
            if !is_valid_agent_name(&name) {
                tracing::warn!(
                    agent = %name,
                    "invalid a2a agent name, skipping (must start with a lowercase letter and \
                     contain only lowercase letters, digits, and underscores, at most \
                     64 characters)"
                );
                return None;
            }
            if raw.url.trim().is_empty() {
                tracing::warn!(agent = %name, "a2a agent has no url, skipping");
                return None;
            }
            let mut headers = HashMap::with_capacity(raw.headers.len());
            for (key, value) in raw.headers {
                let expanded = match crate::agent_keys::expand_references(&value, agent_keys) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            agent = %name,
                            header = %key,
                            error = %e,
                            "a2a agent header references an agent key, skipping agent"
                        );
                        return None;
                    }
                };
                headers.insert(key, crate::mcp::client::expand_env_vars(&expanded));
            }
            Some((
                name.clone(),
                A2aAgentEntry {
                    name,
                    url: raw.url,
                    headers,
                },
            ))
        })
        .collect();

    tracing::debug!(count = agents.len(), "loaded a2a agents");
    Ok(agents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_keys::KeyCreator;

    fn keys_with(name: &str, value: &str) -> AgentKeyStore {
        let mut store = AgentKeyStore::default();
        store.set(name, value, None, KeyCreator::User).unwrap();
        store
    }

    #[test]
    fn name_validation() {
        for good in ["a", "laptop", "second_instance", "x1"] {
            assert!(is_valid_agent_name(good), "'{good}' should be accepted");
        }
        for bad in ["", "Laptop", "1x", "_x", "has-dash", "has space", "a2a:x"] {
            assert!(!is_valid_agent_name(bad), "'{bad}' should be rejected");
        }
        assert!(!is_valid_agent_name(&"a".repeat(MAX_NAME_LEN + 1)));
    }

    #[test]
    fn missing_file_is_empty_map() {
        let dir = tempfile::tempdir().unwrap();
        let map =
            load_a2a_agents_map(&dir.path().join("nope.json"), &AgentKeyStore::default()).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn loads_valid_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(
            &path,
            r#"{"agents": {"laptop": {"url": "https://laptop.example.com/a2a/laptop"}}}"#,
        )
        .unwrap();
        let map = load_a2a_agents_map(&path, &AgentKeyStore::default()).unwrap();
        assert_eq!(map.len(), 1);
        let Some(laptop) = map.get("laptop") else {
            panic!("expected a 'laptop' entry, got {map:?}");
        };
        assert_eq!(laptop.url, "https://laptop.example.com/a2a/laptop");
    }

    #[test]
    fn expands_agent_key_references_in_headers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(
            &path,
            r#"{"agents": {"laptop": {"url": "https://x.example.com", "headers": {"Authorization": "Bearer ${agent-key:laptop_token}"}}}}"#,
        )
        .unwrap();
        let keys = keys_with("laptop_token", "secret123");
        let map = load_a2a_agents_map(&path, &keys).unwrap();
        let Some(header) = map
            .get("laptop")
            .and_then(|e| e.headers.get("Authorization"))
        else {
            panic!("expected an expanded Authorization header, got {map:?}");
        };
        assert_eq!(header, "Bearer secret123");
    }

    #[test]
    fn expands_env_var_references_with_default_in_headers() {
        // Uses the `${VAR:-default}` fallback form so the test needs no
        // process-wide env mutation (which would race with other tests).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(
            &path,
            r#"{"agents": {"laptop": {"url": "https://x.example.com", "headers": {"X-Custom": "${A2A_CLIENT_CONFIG_TEST_VAR_UNSET:-from-default}"}}}}"#,
        )
        .unwrap();
        let map = load_a2a_agents_map(&path, &AgentKeyStore::default()).unwrap();
        let Some(header) = map.get("laptop").and_then(|e| e.headers.get("X-Custom")) else {
            panic!("expected an expanded X-Custom header, got {map:?}");
        };
        assert_eq!(header, "from-default");
    }

    #[test]
    fn skips_entry_with_invalid_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(
            &path,
            r#"{"agents": {"Bad Name": {"url": "https://x.example.com"}, "good": {"url": "https://y.example.com"}}}"#,
        )
        .unwrap();
        let map = load_a2a_agents_map(&path, &AgentKeyStore::default()).unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("good"));
    }

    #[test]
    fn skips_entry_with_empty_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(&path, r#"{"agents": {"laptop": {"url": ""}}}"#).unwrap();
        let map = load_a2a_agents_map(&path, &AgentKeyStore::default()).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn skips_entry_with_unknown_agent_key_reference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(
            &path,
            r#"{"agents": {"laptop": {"url": "https://x.example.com", "headers": {"Authorization": "Bearer ${agent-key:missing}"}}}}"#,
        )
        .unwrap();
        let map = load_a2a_agents_map(&path, &AgentKeyStore::default()).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn invalid_json_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a.json");
        std::fs::write(&path, "not json").unwrap();
        let err = load_a2a_agents_map(&path, &AgentKeyStore::default()).unwrap_err();
        assert!(err.to_string().contains("failed to parse a2a.json"));
    }

    #[test]
    fn validate_accepts_empty_and_well_formed_agents() {
        validate_a2a_agents_json(r#"{"agents": {}}"#).unwrap();
        validate_a2a_agents_json(
            r#"{"agents": {"laptop": {"url": "https://x.example.com", "headers": {"a": "b"}}}}"#,
        )
        .unwrap();
    }

    #[test]
    fn validate_rejects_invalid_json() {
        let err = validate_a2a_agents_json("not json").unwrap_err();
        assert!(err.contains("invalid JSON"));
    }

    #[test]
    fn validate_rejects_bad_name() {
        let err = validate_a2a_agents_json(
            r#"{"agents": {"Bad Name": {"url": "https://x.example.com"}}}"#,
        )
        .unwrap_err();
        assert!(err.contains("Bad Name"));
    }

    #[test]
    fn validate_rejects_empty_url() {
        let err = validate_a2a_agents_json(r#"{"agents": {"laptop": {"url": ""}}}"#).unwrap_err();
        assert!(err.contains("non-empty url"));
    }
}
