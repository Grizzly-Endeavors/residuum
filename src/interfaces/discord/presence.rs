//! Hot-reloadable Discord presence from a workspace TOML file.
//!
//! Reads `PRESENCE.toml` from the workspace root and converts it to serenity
//! presence types.

use std::path::Path;

use serde::Deserialize;
use serenity::gateway::ActivityData;
use serenity::model::user::OnlineStatus;

/// Raw deserialized content of `PRESENCE.toml`.
#[derive(Deserialize, Default)]
pub struct PresenceFile {
    /// Online status: `"online"`, `"idle"`, `"dnd"`, `"invisible"`.
    pub status: Option<String>,
    /// Activity type: `"playing"`, `"watching"`, `"listening"`, `"competing"`.
    pub activity_type: Option<String>,
    /// Activity text shown after the activity type verb.
    pub activity_text: Option<String>,
}

/// Default activity text when none is specified.
const DEFAULT_ACTIVITY_TEXT: &str = "DMs";

/// Load and parse a `PRESENCE.toml` file, returning defaults on any error.
#[must_use]
pub fn load_presence(path: &Path) -> PresenceFile {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read PRESENCE.toml, using defaults");
            return PresenceFile::default();
        }
    };
    match toml::from_str(&content) {
        Ok(pf) => pf,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to parse PRESENCE.toml, using defaults");
            PresenceFile::default()
        }
    }
}

/// Convert a `PresenceFile` to a serenity `OnlineStatus`.
#[must_use]
pub fn to_online_status(pf: &PresenceFile) -> OnlineStatus {
    match pf.status.as_deref() {
        Some("online") | None => OnlineStatus::Online,
        Some("idle") => OnlineStatus::Idle,
        Some("dnd") => OnlineStatus::DoNotDisturb,
        Some("invisible") => OnlineStatus::Invisible,
        Some(other) => {
            tracing::warn!(
                status = other,
                "unknown presence status, defaulting to online"
            );
            OnlineStatus::Online
        }
    }
}

/// Convert a `PresenceFile` to a serenity `ActivityData`.
#[must_use]
pub fn to_activity(pf: &PresenceFile) -> ActivityData {
    let text = pf.activity_text.as_deref().unwrap_or(DEFAULT_ACTIVITY_TEXT);

    match pf.activity_type.as_deref() {
        Some("listening") | None => ActivityData::listening(text),
        Some("playing") => ActivityData::playing(text),
        Some("watching") => ActivityData::watching(text),
        Some("competing") => ActivityData::competing(text),
        Some(other) => {
            tracing::warn!(
                activity_type = other,
                "unknown activity type, defaulting to listening"
            );
            ActivityData::listening(text)
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_toml() {
        let content = r#"
status = "idle"
activity_type = "playing"
activity_text = "with fire"
"#;
        let pf: PresenceFile = toml::from_str(content).unwrap();
        assert_eq!(pf.status.as_deref(), Some("idle"), "status should be idle");
        assert_eq!(
            pf.activity_type.as_deref(),
            Some("playing"),
            "activity_type should be playing"
        );
        assert_eq!(
            pf.activity_text.as_deref(),
            Some("with fire"),
            "activity_text should match"
        );

        let status = to_online_status(&pf);
        assert_eq!(status, OnlineStatus::Idle, "should convert to Idle");

        let activity = to_activity(&pf);
        assert_eq!(activity.name, "with fire", "activity name should match");
    }

    #[test]
    fn parse_empty_file_defaults() {
        let pf: PresenceFile = toml::from_str("").unwrap();
        assert!(pf.status.is_none(), "status should be None");
        assert!(pf.activity_type.is_none(), "activity_type should be None");
        assert!(pf.activity_text.is_none(), "activity_text should be None");

        let status = to_online_status(&pf);
        assert_eq!(status, OnlineStatus::Online, "default should be Online");

        let activity = to_activity(&pf);
        assert_eq!(activity.name, "DMs", "default activity text should be DMs");
    }

    #[test]
    fn parse_invalid_returns_defaults() {
        let path = std::path::PathBuf::from("/tmp/nonexistent_presence_test.toml");
        let pf = load_presence(&path);
        assert!(pf.status.is_none(), "missing file should return default");
    }

    #[test]
    fn unknown_status_defaults_to_online() {
        let pf = PresenceFile {
            status: Some("bananas".to_string()),
            activity_type: None,
            activity_text: None,
        };
        let status = to_online_status(&pf);
        assert_eq!(
            status,
            OnlineStatus::Online,
            "unknown status should default to Online"
        );
    }

    #[test]
    fn unknown_activity_type_defaults_to_listening() {
        let pf = PresenceFile {
            status: None,
            activity_type: Some("dancing".to_string()),
            activity_text: Some("in the rain".to_string()),
        };
        let activity = to_activity(&pf);
        assert_eq!(
            activity.name, "in the rain",
            "text should still be used with unknown type"
        );
    }

    #[test]
    fn all_valid_statuses() {
        for (input, expected) in [
            ("online", OnlineStatus::Online),
            ("idle", OnlineStatus::Idle),
            ("dnd", OnlineStatus::DoNotDisturb),
            ("invisible", OnlineStatus::Invisible),
        ] {
            let pf = PresenceFile {
                status: Some(input.to_string()),
                activity_type: None,
                activity_text: None,
            };
            assert_eq!(
                to_online_status(&pf),
                expected,
                "status '{input}' should map correctly"
            );
        }
    }

    #[test]
    fn all_valid_activity_types() {
        for input in ["playing", "watching", "listening", "competing"] {
            let pf = PresenceFile {
                status: None,
                activity_type: Some(input.to_string()),
                activity_text: Some("test".to_string()),
            };
            let activity = to_activity(&pf);
            assert_eq!(
                activity.name, "test",
                "activity text should be preserved for type '{input}'"
            );
        }
    }

    #[test]
    fn load_presence_invalid_toml_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("PRESENCE.toml");
        std::fs::write(&path, b"this is not valid = toml [[[").unwrap();
        let pf = load_presence(&path);
        assert!(
            pf.status.is_none(),
            "invalid TOML should return default status"
        );
        assert!(
            pf.activity_type.is_none(),
            "invalid TOML should return default activity_type"
        );
        assert!(
            pf.activity_text.is_none(),
            "invalid TOML should return default activity_text"
        );
    }
}
