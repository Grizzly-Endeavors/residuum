//! In-place text edits to HEARTBEAT.yml.
//!
//! Unlike [`super::types::load_heartbeat`], which parses the file into
//! structured data, these edits work on the raw text and touch only the one
//! value being changed — every other byte (comments, blank lines,
//! formatting, other pulses) survives untouched. This is what lets the
//! Scheduled view's enabled toggle flip a single pulse without silently
//! discarding a comment the owner or agent left in the file.

/// One pulse entry's location within the raw text, found by
/// [`find_pulse_entry`].
struct PulseEntry {
    /// Line index of the `- name: ...` marker.
    marker_line: usize,
    /// Line index of an existing `enabled:` field within the entry's block,
    /// if it has one.
    enabled_line: Option<usize>,
    /// Indentation (in spaces) a direct field of this entry uses — derived
    /// from an existing field when the entry has one, otherwise the marker's
    /// own indentation plus two spaces, matching every documented
    /// HEARTBEAT.yml example (`  - name: x` then `    schedule: ...`).
    field_indent: usize,
}

/// Find the pulse named `pulse_name` among `- name: ...` list markers in
/// `lines`, and locate its block's existing `enabled:` field, if any.
fn find_pulse_entry(lines: &[&str], pulse_name: &str) -> Option<PulseEntry> {
    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let Some(rest) = trimmed.strip_prefix("- name:") else {
            continue;
        };
        if strip_yaml_scalar_quotes(rest.trim()) != pulse_name {
            continue;
        }

        let marker_indent = indent;
        let mut enabled_line = None;
        let mut field_indent = marker_indent + 2;
        let mut saw_field_indent = false;

        for (j, &l) in lines.iter().enumerate().skip(i + 1) {
            if l.trim().is_empty() {
                continue;
            }
            let t = l.trim_start();
            let ind = l.len() - t.len();
            // Dedented back to this entry's own indent (the next list
            // item) or shallower: the block has ended.
            if ind <= marker_indent {
                break;
            }
            if !saw_field_indent {
                field_indent = ind;
                saw_field_indent = true;
            }
            if ind == field_indent && t.starts_with("enabled:") {
                enabled_line = Some(j);
            }
        }

        return Some(PulseEntry {
            marker_line: i,
            enabled_line,
            field_indent,
        });
    }
    None
}

/// Strip a leading/trailing single- or double-quote pair from a scalar, the
/// way YAML permits quoting a plain string. Only strips a matching pair of
/// the same quote character at both ends, never a single stray quote or a
/// mismatched pair.
fn strip_yaml_scalar_quotes(s: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = s
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    s
}

/// Flip a named pulse's `enabled` field in raw HEARTBEAT.yml text, editing
/// only that one value (or inserting it, right after the pulse's `name`
/// line, if it has none) and leaving every other line of the file
/// unchanged.
///
/// # Errors
/// Returns an error naming the pulse if no `pulses` entry with that `name`
/// is found in the text.
pub fn set_pulse_enabled(yaml: &str, pulse_name: &str, enabled: bool) -> Result<String, String> {
    let lines: Vec<&str> = yaml.split('\n').collect();
    let entry = find_pulse_entry(&lines, pulse_name)
        .ok_or_else(|| format!("no pulse named \"{pulse_name}\" found in HEARTBEAT.yml"))?;

    let new_field_line = format!("{}enabled: {enabled}", " ".repeat(entry.field_indent));

    let mut result: Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();
    if let Some(idx) = entry.enabled_line {
        let Some(slot) = result.get_mut(idx) else {
            return Err(format!(
                "internal error: enabled line {idx} out of range while editing pulse \
                 \"{pulse_name}\""
            ));
        };
        *slot = new_field_line;
    } else {
        result.insert(entry.marker_line + 1, new_field_line);
    }
    Ok(result.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
pulses:
  - name: reflection
    enabled: true
    schedule: \"7d\"
    agent: introspection
    tasks: []
  # a comment the agent left about memory_tending
  - name: memory_tending
    schedule: \"24h\"
    active_hours: \"02:00-06:00\"
    agent: wiki
    tasks: []
";

    #[test]
    fn replaces_an_existing_enabled_value_in_place() {
        let updated = set_pulse_enabled(SAMPLE, "reflection", false).unwrap();
        assert!(updated.contains("    enabled: false"));
        assert!(
            !updated.contains("    enabled: true"),
            "the old value should be gone, not left alongside the new one"
        );
        // Everything else, including the comment, must survive untouched.
        assert!(updated.contains("# a comment the agent left about memory_tending"));
        assert!(updated.contains("- name: memory_tending"));
    }

    #[test]
    fn inserts_enabled_when_the_pulse_has_none() {
        let updated = set_pulse_enabled(SAMPLE, "memory_tending", false).unwrap();
        let lines: Vec<&str> = updated.lines().collect();
        let name_idx = lines
            .iter()
            .position(|l| l.contains("- name: memory_tending"))
            .unwrap();
        assert_eq!(
            lines.get(name_idx + 1).map(|l| l.trim()),
            Some("enabled: false"),
            "enabled should be inserted right after the pulse's name line"
        );
        // The pulse's other fields must still be present, unchanged.
        assert!(updated.contains("schedule: \"24h\""));
        assert!(updated.contains("active_hours: \"02:00-06:00\""));
    }

    #[test]
    fn only_the_targeted_pulse_is_touched() {
        let updated = set_pulse_enabled(SAMPLE, "memory_tending", false).unwrap();
        assert!(
            updated.contains("    enabled: true"),
            "reflection's own enabled: true must be untouched"
        );
    }

    #[test]
    fn unknown_pulse_name_errors() {
        let err = set_pulse_enabled(SAMPLE, "does_not_exist", true).unwrap_err();
        assert!(err.contains("does_not_exist"));
    }

    #[test]
    fn quoted_pulse_name_still_matches() {
        let yaml = "pulses:\n  - name: \"quoted_name\"\n    schedule: \"1h\"\n    tasks: []\n";
        let updated = set_pulse_enabled(yaml, "quoted_name", false).unwrap();
        assert!(updated.contains("enabled: false"));
    }

    #[test]
    fn preserves_trailing_newline() {
        let updated = set_pulse_enabled(SAMPLE, "reflection", false).unwrap();
        assert!(SAMPLE.ends_with('\n'));
        assert!(updated.ends_with('\n'));
    }

    #[test]
    fn last_pulse_in_file_with_no_trailing_fields_still_gets_enabled_inserted() {
        let yaml = "pulses:\n  - name: only_one\n    schedule: \"1h\"\n    tasks: []\n";
        let updated = set_pulse_enabled(yaml, "only_one", false).unwrap();
        assert!(updated.contains("enabled: false"));
        assert!(updated.contains("schedule: \"1h\""));
    }
}
