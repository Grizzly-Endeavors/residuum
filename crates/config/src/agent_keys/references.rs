//! `${agent-key:<name>}` references in configuration values (MCP `env` and
//! `headers`).

use super::{AgentKeyError, AgentKeyStore};

const OPEN: &str = "${agent-key:";

/// Whether `value` contains any `${agent-key:<name>}` reference.
#[must_use]
pub fn has_references(value: &str) -> bool {
    value.contains(OPEN)
}

/// `value` with every `${agent-key:<name>}` replaced by that key's value.
///
/// # Errors
/// Returns `AgentKeyError::NotFound` for a reference to an unknown key, and
/// `AgentKeyError::Invalid` for an unterminated reference.
pub fn expand_references(value: &str, store: &AgentKeyStore) -> Result<String, AgentKeyError> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find(OPEN) {
        let (before, from_open) = rest.split_at(start);
        out.push_str(before);
        let after_open = from_open.get(OPEN.len()..).unwrap_or_default();
        let Some(end) = after_open.find('}') else {
            return Err(AgentKeyError::Invalid(format!(
                "unterminated agent key reference in '{value}' (expected ${{agent-key:<name>}})"
            )));
        };
        let (name, after_name) = after_open.split_at(end);
        let key_value = store
            .value(name)
            .ok_or_else(|| AgentKeyError::NotFound(name.to_string()))?;
        out.push_str(key_value);
        rest = after_name.get(1..).unwrap_or_default();
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_keys::KeyCreator;

    fn store() -> AgentKeyStore {
        let mut s = AgentKeyStore::default();
        s.set("gh", "ghp_value_123", None, KeyCreator::User)
            .unwrap();
        s
    }

    #[test]
    fn expands_whole_and_inline_references() {
        let s = store();
        assert_eq!(
            expand_references("${agent-key:gh}", &s).unwrap(),
            "ghp_value_123",
            "whole-value reference should expand"
        );
        assert_eq!(
            expand_references("Bearer ${agent-key:gh}", &s).unwrap(),
            "Bearer ghp_value_123",
            "inline reference should expand"
        );
    }

    #[test]
    fn plain_values_pass_through() {
        assert_eq!(
            expand_references("plain ${HOME} value", &store()).unwrap(),
            "plain ${HOME} value",
            "non-agent-key text must be untouched"
        );
        assert!(!has_references("plain"), "no reference detected");
        assert!(has_references("x ${agent-key:gh}"), "reference detected");
    }

    #[test]
    fn unknown_key_is_an_error() {
        assert_eq!(
            expand_references("${agent-key:missing}", &store()),
            Err(AgentKeyError::NotFound("missing".to_string())),
            "unknown key should fail visibly"
        );
    }

    #[test]
    fn unterminated_reference_is_an_error() {
        assert!(
            matches!(
                expand_references("${agent-key:gh", &store()),
                Err(AgentKeyError::Invalid(_))
            ),
            "unterminated reference should be rejected"
        );
    }
}
