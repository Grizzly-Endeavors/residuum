pub(super) fn parse_tolerating_unknown_keys<T: serde::de::DeserializeOwned>(
    contents: &str,
    file_label: &str,
) -> Result<(T, Vec<String>), String> {
    let mut current = contents.to_string();
    let mut notices = Vec::new();

    loop {
        match toml::from_str::<T>(&current) {
            Ok(value) => return Ok((value, notices)),
            Err(err) => {
                if !err.message().starts_with("unknown field") {
                    return Err(format!("{file_label} parse error: {err}"));
                }
                let Some(span) = err.span() else {
                    return Err(format!("{file_label} parse error: {err}"));
                };
                let Some(key) = current.get(span.clone()) else {
                    return Err(format!("{file_label} parse error: {err}"));
                };
                let key = key.to_string();
                let Some(without_key) = remove_unknown_key(&current, span) else {
                    return Err(format!(
                        "{file_label} parse error: {err} (couldn't safely remove it automatically — fix or delete it by hand)"
                    ));
                };
                tracing::warn!(
                    file = file_label,
                    key = %key,
                    "skipping unknown config key"
                );
                notices.push(format!(
                    "Skipped unknown key \"{key}\" in {file_label} — it may have been removed in a newer version, or is a typo. The rest of {file_label} still loaded."
                ));
                current = without_key;
            }
        }
    }
}

fn remove_unknown_key(text: &str, span: std::ops::Range<usize>) -> Option<String> {
    let line_start = text.get(..span.start)?.rfind('\n').map_or(0, |i| i + 1);
    let line_end = text
        .get(span.end..)?
        .find('\n')
        .map_or(text.len(), |i| span.end + i);
    let line = text.get(line_start..line_end)?.trim();

    if line.starts_with('[') {
        let block_end = find_block_end(text, line_end)?;
        return remove_range(text, line_start, block_end);
    }

    if line.contains('{') || line.matches('[').count() != line.matches(']').count() {
        return None;
    }

    remove_range(text, line_start, line_end)
}

fn find_block_end(text: &str, header_line_end: usize) -> Option<usize> {
    let mut pos = header_line_end;
    loop {
        let next_start = if pos < text.len() { pos + 1 } else { pos };
        if next_start >= text.len() {
            return Some(text.len());
        }
        let rest = text.get(next_start..)?;
        let next_end = rest.find('\n').map_or(text.len(), |i| next_start + i);
        let next_line = text.get(next_start..next_end)?;
        if next_line.trim_start().starts_with('[') {
            return Some(pos);
        }
        pos = next_end;
        if pos >= text.len() {
            return Some(text.len());
        }
    }
}

fn remove_range(text: &str, start: usize, end: usize) -> Option<String> {
    let tail = text.get(end..)?;
    let after = if tail.starts_with('\n') { end + 1 } else { end };
    let mut out = String::with_capacity(text.len());
    out.push_str(text.get(..start)?);
    out.push_str(text.get(after..)?);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize, Debug, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Section {
        foo: i32,
    }

    #[derive(serde::Deserialize, Debug, Default, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Root {
        #[serde(default)]
        top: Option<i32>,
        #[serde(default)]
        a: Option<Section>,
    }

    #[test]
    fn drops_one_unknown_scalar_key_within_a_section() {
        let toml_str = "[a]\nfoo = 1\nbar = 2\n";
        let (value, notices) =
            parse_tolerating_unknown_keys::<Root>(toml_str, "config.toml").unwrap();
        assert_eq!(value.a, Some(Section { foo: 1 }));
        assert_eq!(notices.len(), 1);
        assert!(
            notices.first().unwrap().contains("bar"),
            "{}",
            notices.first().unwrap()
        );
        assert!(
            notices.first().unwrap().contains("config.toml"),
            "{}",
            notices.first().unwrap()
        );
    }

    #[test]
    fn drops_an_entirely_unknown_top_level_key() {
        let toml_str = "top = 1\ntranscript_retention_days = 30\n\n[a]\nfoo = 2\n";
        let (value, notices) =
            parse_tolerating_unknown_keys::<Root>(toml_str, "config.toml").unwrap();
        assert_eq!(value.top, Some(1));
        assert_eq!(value.a, Some(Section { foo: 2 }));
        assert_eq!(notices.len(), 1);
        assert!(
            notices
                .first()
                .unwrap()
                .contains("transcript_retention_days")
        );
    }

    #[test]
    fn drops_an_entirely_unknown_table_and_keeps_the_rest() {
        let toml_str = "top = 1\n\n[unknown_section]\nx = 1\ny = 2\n\n[a]\nfoo = 3\n";
        let (value, notices) =
            parse_tolerating_unknown_keys::<Root>(toml_str, "config.toml").unwrap();
        assert_eq!(value.top, Some(1));
        assert_eq!(value.a, Some(Section { foo: 3 }));
        assert_eq!(notices.len(), 1);
        assert!(notices.first().unwrap().contains("unknown_section"));
    }

    #[test]
    fn drops_a_trailing_unknown_table_with_no_following_section() {
        let toml_str = "top = 1\n\n[a]\nfoo = 3\n\n[unknown_section]\nx = 1\ny = 2\n";
        let (value, notices) =
            parse_tolerating_unknown_keys::<Root>(toml_str, "config.toml").unwrap();
        assert_eq!(value.top, Some(1));
        assert_eq!(value.a, Some(Section { foo: 3 }));
        assert_eq!(notices.len(), 1);
        assert!(notices.first().unwrap().contains("unknown_section"));
    }

    #[test]
    fn drops_multiple_unknown_keys_across_retries() {
        let toml_str = "top = 1\nbad1 = true\nbad2 = false\n\n[a]\nfoo = 1\n";
        let (value, notices) =
            parse_tolerating_unknown_keys::<Root>(toml_str, "config.toml").unwrap();
        assert_eq!(value.top, Some(1));
        assert_eq!(value.a, Some(Section { foo: 1 }));
        assert_eq!(notices.len(), 2);
    }

    #[test]
    fn valid_toml_produces_no_notices() {
        let toml_str = "top = 1\n\n[a]\nfoo = 1\n";
        let (value, notices) =
            parse_tolerating_unknown_keys::<Root>(toml_str, "config.toml").unwrap();
        assert_eq!(value.top, Some(1));
        assert!(notices.is_empty(), "expected {notices:?} to be empty");
    }

    #[test]
    fn a_type_error_on_a_known_field_stays_fatal() {
        let toml_str = "[a]\nfoo = \"not-a-number\"\n";
        let result = parse_tolerating_unknown_keys::<Root>(toml_str, "config.toml");
        assert!(result.is_err(), "a type error should not be swallowed");
    }

    #[test]
    fn invalid_toml_syntax_stays_fatal() {
        let toml_str = "this is not valid toml [[[";
        let result = parse_tolerating_unknown_keys::<Root>(toml_str, "config.toml");
        assert!(result.is_err());
    }
}
