//! Server-side patching of `mcp.json`.
//!
//! The Settings web form only models a subset of each server entry's
//! fields (`command`/`args`/`env` for stdio, `url`/`type`/`headers` for
//! HTTP). Rebuilding the whole file from form state drops anything the
//! form doesn't model. [`apply_mcp_patch`] instead merges a JSON diff —
//! only the fields the user changed — into the existing parsed document, so
//! untouched servers, and untouched fields on a touched server, survive
//! byte-for-byte in content (JSON has no comments to preserve).
//!
//! The diff shape mirrors `mcp.json`: `{"mcpServers": {"<name>": {...}}}`.
//! A JSON `null` at a server name removes that server entirely. A JSON
//! `null` at a field removes just that field. Any top-level key besides
//! `mcpServers`, and any per-server field the diff doesn't mention, are left
//! exactly as they were.

use serde_json::{Map, Value};

/// Apply a JSON diff to an existing `mcp.json` text, returning the patched
/// text.
///
/// A missing or empty `existing_text` is treated as `{"mcpServers": {}}`,
/// matching `GET /api/mcp/raw`'s behavior for a file that doesn't exist yet.
///
/// # Errors
/// Returns a plain-language error if `existing_text` is non-empty but not
/// valid JSON, if its top level isn't a JSON object, or if `diff` isn't a
/// JSON object. The existing text is never modified in these cases.
pub fn apply_mcp_patch(existing_text: &str, diff: &Value) -> Result<String, String> {
    let mut doc: Value = if existing_text.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(existing_text)
            .map_err(|e| format!("mcp.json is not valid JSON, so it was left unchanged: {e}"))?
    };

    let Some(doc_map) = doc.as_object_mut() else {
        return Err(
            "mcp.json's top level isn't a JSON object, so it was left unchanged".to_string(),
        );
    };

    let Some(diff_map) = diff.as_object() else {
        return Err("mcp patch must be a JSON object".to_string());
    };

    apply_object_patch(doc_map, diff_map);

    if !doc_map.contains_key("mcpServers") {
        doc_map.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }

    serde_json::to_string_pretty(&doc).map_err(|e| format!("couldn't serialize mcp.json: {e}"))
}

/// Recursively merge a JSON diff object into an existing JSON object.
fn apply_object_patch(doc: &mut Map<String, Value>, diff: &Map<String, Value>) {
    for (key, val) in diff {
        match val {
            Value::Null => {
                doc.remove(key);
            }
            Value::Object(map) => {
                let entry = doc
                    .entry(key.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                if entry.as_object().is_none() {
                    *entry = Value::Object(Map::new());
                }
                let Some(sub) = entry.as_object_mut() else {
                    // Just replaced with `Value::Object` above when it wasn't
                    // one already, so this is unreachable; skip rather than
                    // panic if that invariant ever breaks.
                    continue;
                };
                apply_object_patch(sub, map);
                if sub.is_empty() {
                    doc.remove(key);
                }
            }
            other @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_)) => {
                doc.insert(key.clone(), other.clone());
            }
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    fn j(s: &str) -> Value {
        serde_json::from_str(s).expect("valid json fixture")
    }

    #[test]
    fn unrelated_edit_leaves_an_http_servers_url_type_and_headers_untouched() {
        let existing = r#"{
            "mcpServers": {
                "remote": {
                    "type": "http",
                    "url": "https://mcp.example.com/v1",
                    "headers": {"Authorization": "Bearer abc"}
                },
                "fs": {
                    "command": "mcp-fs"
                }
            }
        }"#;
        let diff = j(r#"{"mcpServers":{"fs":{"command":"mcp-fs-v2"}}}"#);
        let patched = apply_mcp_patch(existing, &diff).unwrap();
        let doc: Value = serde_json::from_str(&patched).unwrap();

        let remote = &doc["mcpServers"]["remote"];
        assert_eq!(remote["type"], "http");
        assert_eq!(remote["url"], "https://mcp.example.com/v1");
        assert_eq!(remote["headers"]["Authorization"], "Bearer abc");
        assert_eq!(doc["mcpServers"]["fs"]["command"], "mcp-fs-v2");
    }

    #[test]
    fn unknown_top_level_key_survives() {
        let existing = r#"{"mcpServers": {}, "someClientExtension": {"foo": "bar"}}"#;
        let diff = j(r#"{"mcpServers":{"fs":{"command":"mcp-fs"}}}"#);
        let patched = apply_mcp_patch(existing, &diff).unwrap();
        let doc: Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(doc["someClientExtension"]["foo"], "bar");
        assert_eq!(doc["mcpServers"]["fs"]["command"], "mcp-fs");
    }

    #[test]
    fn unknown_field_on_a_server_survives_an_unrelated_field_edit() {
        let existing = r#"{
            "mcpServers": {
                "fs": {
                    "command": "mcp-fs",
                    "cwd": "/home/user/project"
                }
            }
        }"#;
        let diff = j(r#"{"mcpServers":{"fs":{"command":"mcp-fs-v2"}}}"#);
        let patched = apply_mcp_patch(existing, &diff).unwrap();
        let doc: Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(doc["mcpServers"]["fs"]["cwd"], "/home/user/project");
        assert_eq!(doc["mcpServers"]["fs"]["command"], "mcp-fs-v2");
    }

    #[test]
    fn null_removes_a_server_entirely() {
        let existing =
            r#"{"mcpServers": {"fs": {"command": "mcp-fs"}, "git": {"command": "mcp-git"}}}"#;
        let diff = j(r#"{"mcpServers":{"fs":null}}"#);
        let patched = apply_mcp_patch(existing, &diff).unwrap();
        let doc: Value = serde_json::from_str(&patched).unwrap();
        assert!(doc["mcpServers"].get("fs").is_none());
        assert_eq!(doc["mcpServers"]["git"]["command"], "mcp-git");
    }

    #[test]
    fn null_removes_just_one_field() {
        let existing = r#"{"mcpServers": {"remote": {"type": "http", "url": "https://a", "headers": {"X": "1"}}}}"#;
        let diff = j(r#"{"mcpServers":{"remote":{"headers":null}}}"#);
        let patched = apply_mcp_patch(existing, &diff).unwrap();
        let doc: Value = serde_json::from_str(&patched).unwrap();
        assert!(doc["mcpServers"]["remote"].get("headers").is_none());
        assert_eq!(doc["mcpServers"]["remote"]["url"], "https://a");
    }

    #[test]
    fn adding_a_new_server_leaves_others_untouched() {
        let existing = r#"{"mcpServers": {"fs": {"command": "mcp-fs"}}}"#;
        let diff = j(r#"{"mcpServers":{"remote":{"type":"http","url":"https://b"}}}"#);
        let patched = apply_mcp_patch(existing, &diff).unwrap();
        let doc: Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(doc["mcpServers"]["fs"]["command"], "mcp-fs");
        assert_eq!(doc["mcpServers"]["remote"]["url"], "https://b");
    }

    #[test]
    fn missing_file_is_treated_as_empty_server_map() {
        let diff = j(r#"{"mcpServers":{"fs":{"command":"mcp-fs"}}}"#);
        let patched = apply_mcp_patch("", &diff).unwrap();
        let doc: Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(doc["mcpServers"]["fs"]["command"], "mcp-fs");
    }

    #[test]
    fn unparseable_existing_file_is_refused_and_untouched() {
        let existing = "not valid json {{{";
        let diff = j(r#"{"mcpServers":{"fs":{"command":"mcp-fs"}}}"#);
        let err = apply_mcp_patch(existing, &diff).unwrap_err();
        assert!(err.contains("mcp.json"), "got: {err}");
        assert!(err.contains("not valid JSON"), "got: {err}");
    }
}
