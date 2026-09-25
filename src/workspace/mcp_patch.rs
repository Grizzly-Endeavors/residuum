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
//! exactly as they were, including their original position in the file —
//! [`OrderedValue`] parses and re-serializes the existing document without
//! going through `serde_json::Value`, whose `Map` sorts keys alphabetically
//! unless the crate's `preserve_order` feature is on. That feature is
//! unified across the whole binary by Cargo, so enabling it here would
//! reorder every other JSON document residuum serializes; this type keeps
//! order-preservation scoped to this one patch path instead.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{Map, Number, Value};

/// A JSON value whose object keys keep the order they appeared in the
/// source text, rather than the alphabetical order `serde_json::Value`
/// imposes by default. See the module docs for why this exists instead of
/// enabling `serde_json`'s `preserve_order` feature.
#[derive(Debug, Clone, PartialEq)]
enum OrderedValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<OrderedValue>),
    /// Insertion-ordered key/value pairs; never has two entries with the
    /// same key.
    Object(Vec<(String, OrderedValue)>),
}

impl OrderedValue {
    fn as_object_mut(&mut self) -> Option<&mut Vec<(String, OrderedValue)>> {
        match self {
            Self::Object(entries) => Some(entries),
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_) | Self::Array(_) => None,
        }
    }
}

impl From<Value> for OrderedValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(b),
            Value::Number(n) => Self::Number(n),
            Value::String(s) => Self::String(s),
            Value::Array(arr) => Self::Array(arr.into_iter().map(Self::from).collect()),
            Value::Object(map) => {
                Self::Object(map.into_iter().map(|(k, v)| (k, Self::from(v))).collect())
            }
        }
    }
}

impl<'de> Deserialize<'de> for OrderedValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OrderedValueVisitor;

        impl<'de> Visitor<'de> for OrderedValueVisitor {
            type Value = OrderedValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
                Ok(OrderedValue::Bool(v))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(OrderedValue::Number(v.into()))
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(OrderedValue::Number(v.into()))
            }

            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
                Ok(Number::from_f64(v).map_or(OrderedValue::Null, OrderedValue::Number))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(OrderedValue::String(v.to_string()))
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
                Ok(OrderedValue::String(v))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(OrderedValue::Null)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(OrderedValue::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                Deserialize::deserialize(deserializer)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(elem) = seq.next_element()? {
                    vec.push(elem);
                }
                Ok(OrderedValue::Array(vec))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries: Vec<(String, OrderedValue)> = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, OrderedValue>()? {
                    if let Some(existing) = entries.iter_mut().find(|(key, _)| *key == k) {
                        existing.1 = v;
                    } else {
                        entries.push((k, v));
                    }
                }
                Ok(OrderedValue::Object(entries))
            }
        }

        deserializer.deserialize_any(OrderedValueVisitor)
    }
}

impl Serialize for OrderedValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Number(n) => n.serialize(serializer),
            Self::String(s) => serializer.serialize_str(s),
            Self::Array(arr) => {
                let mut seq = serializer.serialize_seq(Some(arr.len()))?;
                for v in arr {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            Self::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (k, v) in entries {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

fn get_mut<'a>(
    entries: &'a mut [(String, OrderedValue)],
    key: &str,
) -> Option<&'a mut OrderedValue> {
    entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn remove(entries: &mut Vec<(String, OrderedValue)>, key: &str) {
    entries.retain(|(k, _)| k != key);
}

fn set(entries: &mut Vec<(String, OrderedValue)>, key: &str, value: OrderedValue) {
    if let Some(existing) = get_mut(entries, key) {
        *existing = value;
    } else {
        entries.push((key.to_string(), value));
    }
}

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
    let mut doc: OrderedValue = if existing_text.trim().is_empty() {
        OrderedValue::Object(Vec::new())
    } else {
        serde_json::from_str(existing_text)
            .map_err(|e| format!("mcp.json is not valid JSON, so it was left unchanged: {e}"))?
    };

    let Some(doc_entries) = doc.as_object_mut() else {
        return Err(
            "mcp.json's top level isn't a JSON object, so it was left unchanged".to_string(),
        );
    };

    let Some(diff_map) = diff.as_object() else {
        return Err("mcp patch must be a JSON object".to_string());
    };

    apply_object_patch(doc_entries, diff_map);

    if get_mut(doc_entries, "mcpServers").is_none() {
        doc_entries.push(("mcpServers".to_string(), OrderedValue::Object(Vec::new())));
    }

    serde_json::to_string_pretty(&doc).map_err(|e| format!("couldn't serialize mcp.json: {e}"))
}

/// Recursively merge a JSON diff object into an existing ordered object.
fn apply_object_patch(doc: &mut Vec<(String, OrderedValue)>, diff: &Map<String, Value>) {
    for (key, val) in diff {
        match val {
            Value::Null => remove(doc, key),
            Value::Object(map) => {
                // Existing keys keep their position; a key that's new, or
                // whose existing value isn't itself an object, is reset to
                // an empty one (matching the un-ordered version's
                // `.or_insert_with` behavior) before merging into it.
                if !matches!(get_mut(doc, key), Some(OrderedValue::Object(_))) {
                    set(doc, key, OrderedValue::Object(Vec::new()));
                }
                let Some(OrderedValue::Object(sub)) = get_mut(doc, key) else {
                    // Just ensured it's an `Object` above, so this is
                    // unreachable; skip rather than panic if that
                    // invariant ever breaks.
                    continue;
                };
                apply_object_patch(sub, map);
                if sub.is_empty() {
                    remove(doc, key);
                }
            }
            other @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_)) => {
                set(doc, key, OrderedValue::from(other.clone()));
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

    #[test]
    fn original_key_order_is_preserved_across_a_patch() {
        // Keys deliberately out of alphabetical order; a `serde_json::Value`
        // round-trip (without `preserve_order`) would re-sort them to
        // fs, git, remote and reorder each server's own fields too.
        let existing = r#"{
            "mcpServers": {
                "zeta": {"command": "mcp-zeta"},
                "alpha": {"url": "https://a", "type": "http", "headers": {}}
            },
            "trailingExtension": {"note": "keep me last"}
        }"#;
        let diff = j(r#"{"mcpServers":{"zeta":{"command":"mcp-zeta-v2"}}}"#);
        let patched = apply_mcp_patch(existing, &diff).unwrap();

        let top_level_keys: Vec<&str> = extract_object_keys(&patched, 0, 0);
        assert_eq!(top_level_keys, vec!["mcpServers", "trailingExtension"]);

        let server_keys: Vec<&str> = extract_object_keys(&patched, 1, 0);
        assert_eq!(server_keys, vec!["zeta", "alpha"]);

        // alpha is the second object at depth 2 (zeta's fields come first).
        let alpha_field_keys: Vec<&str> = extract_object_keys(&patched, 2, 1);
        assert_eq!(alpha_field_keys, vec!["url", "type", "headers"]);
    }

    /// Extract the ordered keys of the `occurrence`-th `{...}` object at
    /// nesting `depth` in `text` (depth 0 = the top-level object; occurrence
    /// 0 = the first one at that depth, occurrence 1 = the next sibling at
    /// the same depth, etc.), by scanning the raw text directly rather than
    /// through an order-sorting JSON parser: tracks `{`/`}` nesting and
    /// skips over string literals (including escapes) so brackets inside
    /// values don't confuse the depth count.
    fn extract_object_keys(text: &str, depth: usize, occurrence: usize) -> Vec<&str> {
        #[derive(PartialEq)]
        enum State {
            /// Haven't reached the target object yet.
            Searching,
            /// Inside the target object; its top-level keys are collected.
            Collecting,
            /// The target object has closed; ignore anything after it.
            Done,
        }

        let target_depth = i32::try_from(depth).expect("test depth fits i32");
        let bytes = text.as_bytes();
        let mut i = 0_usize;
        let mut obj_depth: i32 = -1;
        let mut seen_at_depth = 0_usize;
        let mut state = State::Searching;
        let mut keys = Vec::new();
        let mut expect_key = false;

        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    let key_start = i + 1;
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    let key_end = i;
                    if expect_key && state == State::Collecting && obj_depth == target_depth {
                        keys.push(text.get(key_start..key_end).unwrap_or_default());
                    }
                    expect_key = false;
                }
                b'{' => {
                    obj_depth += 1;
                    if state == State::Searching && obj_depth == target_depth {
                        if seen_at_depth == occurrence {
                            state = State::Collecting;
                        }
                        seen_at_depth += 1;
                    }
                    expect_key = state == State::Collecting && obj_depth == target_depth;
                }
                b'}' => {
                    if state == State::Collecting && obj_depth == target_depth {
                        state = State::Done;
                    }
                    obj_depth -= 1;
                    expect_key = false;
                }
                b',' => {
                    expect_key = state == State::Collecting && obj_depth == target_depth;
                }
                _ => {}
            }
            i += 1;
        }
        keys
    }
}
