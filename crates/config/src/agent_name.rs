use toml_edit::DocumentMut;
use unicode_normalization::UnicodeNormalization;

use super::paths::{MAX_AGENT_NAME_LEN, validate_agent_name};

pub const MAX_DISPLAY_NAME_CHARS: usize = 32;

const RESERVED_KEYS: &[&str] = &["hub", "team", "agents"];

const LATIN_ASCII: &[(char, &str)] = &[
    ('ß', "ss"),
    ('æ', "ae"),
    ('ø', "o"),
    ('ł', "l"),
    ('đ', "d"),
    ('ð', "d"),
    ('þ', "th"),
    ('œ', "oe"),
    ('ı', "i"),
    ('ŋ', "ng"),
];

pub fn canonicalize_display_name(raw: &str) -> Result<String, String> {
    let name: String = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .nfc()
        .collect();
    if name.is_empty() {
        return Err("agent name must not be empty".to_string());
    }
    if name.chars().count() > MAX_DISPLAY_NAME_CHARS {
        return Err(format!(
            "agent name '{name}' is too long: at most {MAX_DISPLAY_NAME_CHARS} characters"
        ));
    }
    if name.starts_with(['-', '\'']) || name.ends_with(['-', '\'']) {
        return Err(format!(
            "agent name '{name}' can't start or end with a hyphen or an apostrophe"
        ));
    }
    let allowed = name
        .chars()
        .all(|c| c.is_alphabetic() || c.is_numeric() || c == ' ' || c == '-' || c == '\'');
    if !allowed || !name.chars().any(|c| c.is_alphabetic() || c.is_numeric()) {
        return Err(format!(
            "agent name '{name}' can use letters, numbers, spaces, hyphens, and apostrophes"
        ));
    }
    let key = display_name_key(&name);
    if RESERVED_KEYS.contains(&key.as_str()) {
        return Err(format!(
            "agent name '{name}' is reserved and cannot be used; reserved names: {}",
            RESERVED_KEYS.join(", ")
        ));
    }
    Ok(name)
}

#[must_use]
pub fn display_name_key(canonical: &str) -> String {
    canonical.to_lowercase()
}

#[must_use]
pub fn slug_base(display: &str) -> String {
    let key = display_name_key(display);
    let ascii = fold_to_ascii(&key);
    if ascii.is_empty() {
        hash_slug(&key)
    } else {
        ascii
    }
}

pub fn allocate_slug(base: &str, mut taken: impl FnMut(&str) -> bool) -> Result<String, String> {
    if validate_agent_name(base).is_ok() && !taken(base) {
        return Ok(base.to_string());
    }
    for n in 2..1000_u32 {
        let suffix = format!("-{n}");
        let room = MAX_AGENT_NAME_LEN.saturating_sub(suffix.len());
        let mut stem: String = base.chars().take(room).collect();
        while stem.ends_with('-') {
            stem.pop();
        }
        if stem.is_empty() {
            continue;
        }
        let candidate = format!("{stem}{suffix}");
        if validate_agent_name(&candidate).is_ok() && !taken(&candidate) {
            return Ok(candidate);
        }
    }
    Err("couldn't find a free folder for that name. Try a different one.".to_string())
}

#[must_use]
pub fn display_name_in_toml(config: &str, fallback: &str) -> String {
    let Ok(doc) = config.parse::<DocumentMut>() else {
        return fallback.to_string();
    };
    match doc.get("display_name").and_then(toml_edit::Item::as_str) {
        Some(raw) => canonicalize_display_name(raw).unwrap_or_else(|_| fallback.to_string()),
        None => fallback.to_string(),
    }
}

pub fn set_display_name_toml(config: &str, display: &str) -> Result<String, String> {
    let mut doc = config
        .parse::<DocumentMut>()
        .map_err(|e| format!("config.toml parse error: {e}"))?;
    let _replaced = doc
        .as_table_mut()
        .insert("display_name", toml_edit::value(display));
    Ok(doc.to_string())
}

fn fold_to_ascii(lowercased: &str) -> String {
    let mut raw = String::new();
    for ch in lowercased.nfd() {
        if is_combining_mark(ch) {
            continue;
        }
        if ch.is_ascii_alphanumeric() {
            raw.push(ch);
        } else if ch == ' ' || ch == '-' {
            raw.push('-');
        } else if ch == '\'' {
        } else if let Some((_, ascii)) = LATIN_ASCII.iter().find(|(letter, _)| *letter == ch) {
            raw.push_str(ascii);
        }
    }
    let mut collapsed = String::new();
    for ch in raw.chars() {
        if ch == '-' && collapsed.ends_with('-') {
            continue;
        }
        collapsed.push(ch);
    }
    let trimmed = collapsed.trim_matches('-');
    let mut fitted: String = trimmed.chars().take(MAX_AGENT_NAME_LEN).collect();
    while fitted.ends_with('-') {
        fitted.pop();
    }
    fitted
}

fn is_combining_mark(ch: char) -> bool {
    matches!(
        ch,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn hash_slug(key: &str) -> String {
    format!("n{:08x}", fnv1a64(key.as_bytes()) & 0xffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_in_toml_reads_the_typed_name() {
        assert_eq!(
            display_name_in_toml("display_name = \"Mist\"\n", "mist"),
            "Mist"
        );
        assert_eq!(display_name_in_toml("", "mist"), "mist");
        assert_eq!(display_name_in_toml("not toml", "mist"), "mist");
        assert_eq!(
            display_name_in_toml("display_name = \"nope!\"\n", "mist"),
            "mist"
        );
    }

    #[test]
    fn capitals_spaces_and_other_languages_are_names() {
        for good in [
            "Atlas",
            "Research Desk",
            "José",
            "O'Brien",
            "研究助手",
            "Agent 2",
        ] {
            assert!(canonicalize_display_name(good).is_ok(), "{good}");
        }
    }

    #[test]
    fn cleanup_collapses_space_and_composes_accents() {
        assert_eq!(
            canonicalize_display_name("  Research   Desk  ").unwrap(),
            "Research Desk"
        );
        assert_eq!(canonicalize_display_name("Jose\u{0301}").unwrap(), "José");
    }

    #[test]
    fn case_is_the_same_agent() {
        let atlas = canonicalize_display_name("Atlas").unwrap();
        let lower = canonicalize_display_name("atlas").unwrap();
        assert_eq!(display_name_key(&atlas), display_name_key(&lower));
        assert_ne!(atlas, lower);
    }

    #[test]
    fn rejected_names_say_why() {
        assert!(
            canonicalize_display_name("   ")
                .unwrap_err()
                .contains("empty")
        );
        assert!(
            canonicalize_display_name(&"x".repeat(33))
                .unwrap_err()
                .contains("too long")
        );
        assert!(
            canonicalize_display_name("nope!")
                .unwrap_err()
                .contains("apostrophes")
        );
        assert!(
            canonicalize_display_name("-lead")
                .unwrap_err()
                .contains("hyphen")
        );
        assert!(
            canonicalize_display_name("Hub")
                .unwrap_err()
                .contains("reserved")
        );
    }

    #[test]
    fn slugs_keep_letters_and_drop_accents() {
        assert_eq!(slug_base("Research Desk"), "research-desk");
        assert_eq!(slug_base("Atlas"), "atlas");
        assert_eq!(slug_base("José"), "jose");
        assert_eq!(slug_base("O'Brien"), "obrien");
        assert_eq!(slug_base("Straße"), "strasse");
    }

    #[test]
    fn a_name_with_no_ascii_gets_a_stable_slug() {
        assert_eq!(slug_base("研究"), "n310509ec");
    }

    #[test]
    fn a_taken_slug_grows_a_suffix_that_still_fits() {
        let slug =
            allocate_slug("research-desk", |candidate| candidate == "research-desk").unwrap();
        assert_eq!(slug, "research-desk-2");
        let long = "a".repeat(24);
        let next = allocate_slug(&long, |_| false);
        assert_eq!(next.unwrap(), long);
        let suffixed = allocate_slug(&long, |candidate| candidate == long).unwrap();
        assert_eq!(suffixed.len(), 24, "{suffixed}");
        assert!(suffixed.ends_with("-2"), "{suffixed}");
        assert!(validate_agent_name(&suffixed).is_ok());
    }

    #[test]
    fn a_reserved_folder_name_takes_the_next_slug() {
        let base = slug_base("Hüb");
        assert_eq!(base, "hub");
        let slug = allocate_slug(&base, |_| false).unwrap();
        assert_eq!(slug, "hub-2");
    }
}
