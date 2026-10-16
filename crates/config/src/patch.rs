//! Server-side patching of `config.toml` / `providers.toml`.
//!
//! The Settings web form only models a subset of each file's keys. Rather
//! than rebuilding the whole file from form state (which silently drops
//! anything the form doesn't model — comments, unmodeled sections like
//! `[tracing]`, unmodeled keys), the form sends a diff: a JSON object whose
//! shape mirrors the TOML section/key layout, containing only the fields the
//! user actually changed. [`apply_patch`] applies that diff to the existing
//! file's parsed `toml_edit` document in place, so everything untouched by
//! the diff — formatting, comments, ordering, unmodeled keys — survives.
//!
//! A JSON object value at a key recurses into (or creates) the matching
//! TOML table. A JSON `null` removes that key (or, for a table, removes the
//! whole sub-table) from the document. A JSON object of the shape
//! `{"$inline": {...}}` sets the key to a TOML inline table built from the
//! inner object, rather than recursing — used for model-role assignments
//! that carry `temperature`/`thinking` overrides (`main = { model = "...",
//! temperature = 0.7 }`). Any other JSON scalar or array sets the key to the
//! matching TOML value.

use serde_json::{Map, Value as JsonValue};
use toml_edit::{Array, DocumentMut, InlineTable, Item, TableLike, Value as TomlValue};

/// Apply a JSON diff to an existing TOML document's text, returning the
/// patched text.
///
/// `label` names the file in error messages (e.g. `"config.toml"`).
///
/// # Errors
/// Returns a plain-language error naming `label` and the problem if the
/// existing text is not valid TOML, or if the diff cannot be applied (an
/// object key without a string counterpart, or an unsupported nested
/// shape). The existing text is never consulted for its *semantic*
/// validity here — callers run their normal validation on the result.
pub fn apply_patch(
    existing_text: &str,
    diff: &Map<String, JsonValue>,
    label: &str,
) -> Result<String, String> {
    let mut doc = existing_text
        .parse::<DocumentMut>()
        .map_err(|e| format!("{label} is not valid TOML, so it was left unchanged: {e}"))?;

    apply_table_patch(doc.as_table_mut(), diff)
        .map_err(|e| format!("couldn't apply the change to {label}: {e}"))?;

    Ok(doc.to_string())
}

/// Recursively apply a JSON patch object onto a TOML table-like value.
fn apply_table_patch(
    table: &mut dyn TableLike,
    diff: &Map<String, JsonValue>,
) -> Result<(), String> {
    for (key, val) in diff {
        match val {
            JsonValue::Null => {
                table.remove(key);
            }
            JsonValue::Object(map) => {
                if let Some(JsonValue::Object(inline)) = map.get("$inline") {
                    if map.len() != 1 {
                        return Err(format!(
                            "\"{key}\" mixes $inline with other keys, which isn't a supported patch shape"
                        ));
                    }
                    let inline_table = build_inline_table(inline)?;
                    set_item(table, key, toml_edit::value(inline_table));
                    continue;
                }

                let existing_is_table = table
                    .get(key)
                    .is_some_and(|item| item.as_table_like().is_some());
                if !existing_is_table {
                    set_item(table, key, toml_edit::table());
                }
                let Some(sub) = table.get_mut(key).and_then(Item::as_table_like_mut) else {
                    return Err(format!(
                        "\"{key}\" already holds a value that isn't a table"
                    ));
                };
                apply_table_patch(sub, map)?;

                if sub.is_empty() {
                    table.remove(key);
                }
            }
            JsonValue::Array(items) => {
                let arr = build_array(items)?;
                set_item(table, key, toml_edit::value(arr));
            }
            scalar @ (JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_)) => {
                let toml_val = json_scalar_to_toml(scalar)?;
                set_item(table, key, toml_edit::value(toml_val));
            }
        }
    }
    Ok(())
}

/// Set `key` to `item`, preferring an in-place swap of an existing item over
/// [`TableLike::insert`].
///
/// `insert` reformats the key when it already exists (`Key::fmt` clears its
/// decor), which silently drops a comment sitting on the line above that
/// key. Swapping the existing `Item` in place leaves the key, and its
/// decor, untouched.
fn set_item(table: &mut dyn TableLike, key: &str, item: Item) {
    if let Some(existing) = table.get_mut(key) {
        *existing = item;
    } else {
        table.insert(key, item);
    }
}

/// Build a flat TOML inline table from a JSON object of scalars, for the
/// `{"$inline": {...}}` convention (model-role assignments with overrides).
fn build_inline_table(map: &Map<String, JsonValue>) -> Result<InlineTable, String> {
    let mut inline = InlineTable::new();
    for (key, val) in map {
        if val.is_null() {
            continue;
        }
        let toml_val = json_scalar_to_toml(val)?;
        inline.insert(key, toml_val);
    }
    Ok(inline)
}

/// Build a TOML array from a JSON array. Only scalar elements are supported
/// — every array the settings form emits is a flat string list.
fn build_array(items: &[JsonValue]) -> Result<Array, String> {
    let mut arr = Array::new();
    for item in items {
        arr.push(json_scalar_to_toml(item)?);
    }
    Ok(arr)
}

/// Convert a JSON scalar (string, bool, or number) to a TOML value.
fn json_scalar_to_toml(val: &JsonValue) -> Result<TomlValue, String> {
    match val {
        JsonValue::String(s) => Ok(TomlValue::from(s.as_str())),
        JsonValue::Bool(b) => Ok(TomlValue::from(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(TomlValue::from(i))
            } else if let Some(f) = n.as_f64() {
                Ok(TomlValue::from(f))
            } else {
                Err(format!("number {n} doesn't fit in a TOML int or float"))
            }
        }
        other @ (JsonValue::Null | JsonValue::Array(_) | JsonValue::Object(_)) => {
            Err(format!("unsupported patch value: {other}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(json: &str) -> Map<String, JsonValue> {
        match serde_json::from_str::<JsonValue>(json).expect("valid json fixture") {
            JsonValue::Object(map) => map,
            other @ (JsonValue::Null
            | JsonValue::Bool(_)
            | JsonValue::Number(_)
            | JsonValue::String(_)
            | JsonValue::Array(_)) => panic!("expected a JSON object, got {other}"),
        }
    }

    #[test]
    fn preserves_comments_around_a_patched_key() {
        let existing =
            "# a comment about the gateway\n[gateway]\n# and one about the port\nport = 7700\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"gateway":{"port":8080}}"#),
            "config.toml",
        )
        .unwrap();
        assert!(patched.contains("# a comment about the gateway"));
        assert!(patched.contains("# and one about the port"));
        assert!(patched.contains("port = 8080"));
    }

    #[test]
    fn preserves_an_unmodeled_section() {
        let existing = "[tracing]\nlog_level = \"trace\"\n\n[gateway]\nport = 7700\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"gateway":{"port":8080}}"#),
            "config.toml",
        )
        .unwrap();
        assert!(patched.contains("[tracing]"));
        assert!(patched.contains("log_level = \"trace\""));
        assert!(patched.contains("port = 8080"));
    }

    #[test]
    fn preserves_an_unmodeled_key_in_a_patched_section() {
        let existing = "[gateway]\nbind = \"127.0.0.1\"\nport = 7700\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"gateway":{"port":8080}}"#),
            "config.toml",
        )
        .unwrap();
        assert!(patched.contains("bind = \"127.0.0.1\""));
        assert!(patched.contains("port = 8080"));
    }

    #[test]
    fn null_removes_a_key() {
        let existing = "[discord]\ntoken = \"abc\"\nrespond_to_others = true\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"discord":{"token":null}}"#),
            "config.toml",
        )
        .unwrap();
        assert!(!patched.contains("token"));
        assert!(patched.contains("respond_to_others = true"));
    }

    #[test]
    fn null_removes_a_section_once_emptied() {
        let existing = "[pulse]\nenabled = false\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"pulse":{"enabled":null}}"#),
            "config.toml",
        )
        .unwrap();
        assert!(!patched.contains("[pulse]"));
    }

    #[test]
    fn creates_a_new_section_when_absent() {
        let existing = "name = \"bear\"\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"gateway":{"port":8080}}"#),
            "config.toml",
        )
        .unwrap();
        assert!(patched.contains("[gateway]"));
        assert!(patched.contains("port = 8080"));
    }

    #[test]
    fn inline_marker_builds_an_inline_table_for_a_model_role() {
        let existing = "[models]\nmain = \"claude-opus\"\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"models":{"main":{"$inline":{"model":"claude-sonnet","temperature":0.7}}}}"#),
            "providers.toml",
        )
        .unwrap();
        assert!(patched.contains("model = \"claude-sonnet\""));
        assert!(patched.contains("temperature = 0.7"));
    }

    #[test]
    fn plain_string_replaces_an_inline_table_model_role() {
        let existing = "[models]\nmain = { model = \"claude-sonnet\", temperature = 0.7 }\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"models":{"main":"claude-opus"}}"#),
            "providers.toml",
        )
        .unwrap();
        assert!(patched.contains("main = \"claude-opus\""));
        assert!(!patched.contains("temperature"));
    }

    #[test]
    fn removing_a_named_entry_leaves_siblings_untouched() {
        let existing = "[providers.openai]\ntype = \"openai\"\napi_key = \"sk-1\"\n\n[providers.anthropic]\ntype = \"anthropic\"\napi_key = \"sk-2\"\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"providers":{"openai":null}}"#),
            "providers.toml",
        )
        .unwrap();
        assert!(!patched.contains("openai"));
        assert!(patched.contains("anthropic"));
        assert!(patched.contains("sk-2"));
    }

    #[test]
    fn adding_a_named_entry_leaves_siblings_untouched() {
        let existing = "[providers.anthropic]\ntype = \"anthropic\"\napi_key = \"sk-2\"\n";
        let patched = apply_patch(
            existing,
            &diff(r#"{"providers":{"openai":{"type":"openai","api_key":"sk-1"}}}"#),
            "providers.toml",
        )
        .unwrap();
        assert!(patched.contains("[providers.openai]"));
        assert!(patched.contains("sk-1"));
        assert!(patched.contains("[providers.anthropic]"));
        assert!(patched.contains("sk-2"));
    }

    #[test]
    fn unparseable_existing_file_is_refused_and_untouched() {
        let existing = "this is not [[[ valid toml";
        let err = apply_patch(
            existing,
            &diff(r#"{"gateway":{"port":8080}}"#),
            "config.toml",
        )
        .unwrap_err();
        assert!(err.contains("config.toml"), "got: {err}");
        assert!(err.contains("not valid TOML"), "got: {err}");
    }

    #[test]
    fn string_array_round_trips() {
        let existing = "";
        let patched = apply_patch(
            existing,
            &diff(r#"{"skills":{"dirs":["a","b"]}}"#),
            "config.toml",
        )
        .unwrap();
        assert!(patched.contains("dirs = [\"a\", \"b\"]"));
    }
}
