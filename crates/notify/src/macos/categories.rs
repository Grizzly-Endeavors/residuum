//! Notification categories, interruption levels, and action definitions.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The single `UNNotificationCategory` Residuum registers.
///
/// macOS silently drops action buttons for notifications whose category is not
/// registered, so the identifier used when posting must match the one
/// registered at startup. One category is enough: every Residuum notification
/// carries the same Open/Dismiss actions.
pub const NOTIFICATION_CATEGORY_ID: &str = "background-results";

/// Maps to macOS `UNNotificationInterruptionLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosInterruptionLevel {
    Passive,
    Active,
    /// Breaks through Focus modes (requires entitlement).
    TimeSensitive,
}

impl MacosInterruptionLevel {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Active => "active",
            Self::TimeSensitive => "time_sensitive",
        }
    }
}

impl fmt::Display for MacosInterruptionLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MacosInterruptionLevel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "passive" => Ok(Self::Passive),
            "active" => Ok(Self::Active),
            "time_sensitive" => Ok(Self::TimeSensitive),
            _ => anyhow::bail!("unknown macOS notification priority: {s}"),
        }
    }
}

/// Action buttons displayed on notification banners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosNotificationAction {
    Open,
    Dismiss,
}

impl MacosNotificationAction {
    #[must_use]
    pub fn action_id(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Dismiss => "dismiss",
        }
    }

    #[must_use]
    pub fn button_title(&self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Dismiss => "Dismiss",
        }
    }

    #[must_use]
    pub fn default_actions() -> &'static [Self] {
        &[Self::Open, Self::Dismiss]
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    // ── MacosInterruptionLevel tests ────────────────────────────────────

    #[test]
    fn interruption_level_serde_roundtrip() {
        let levels = [
            MacosInterruptionLevel::Passive,
            MacosInterruptionLevel::Active,
            MacosInterruptionLevel::TimeSensitive,
        ];
        for level in &levels {
            let json =
                serde_json::to_string(level).unwrap_or_else(|_| String::from("serialize failed"));
            let parsed: MacosInterruptionLevel =
                serde_json::from_str(&json).unwrap_or(MacosInterruptionLevel::Active);
            assert_eq!(*level, parsed, "roundtrip failed for {level}");
        }
    }

    #[test]
    fn interruption_level_snake_case_serialization() {
        assert_eq!(
            serde_json::to_string(&MacosInterruptionLevel::TimeSensitive).unwrap_or_default(),
            "\"time_sensitive\""
        );
        assert_eq!(
            serde_json::to_string(&MacosInterruptionLevel::Active).unwrap_or_default(),
            "\"active\""
        );
        assert_eq!(
            serde_json::to_string(&MacosInterruptionLevel::Passive).unwrap_or_default(),
            "\"passive\""
        );
    }

    #[test]
    fn interruption_level_display() {
        assert_eq!(MacosInterruptionLevel::Passive.to_string(), "passive");
        assert_eq!(MacosInterruptionLevel::Active.to_string(), "active");
        assert_eq!(
            MacosInterruptionLevel::TimeSensitive.to_string(),
            "time_sensitive"
        );
    }

    #[test]
    fn parse_priority_valid() {
        assert_eq!(
            "passive"
                .parse::<MacosInterruptionLevel>()
                .unwrap_or(MacosInterruptionLevel::Active),
            MacosInterruptionLevel::Passive
        );
        assert_eq!(
            "active"
                .parse::<MacosInterruptionLevel>()
                .unwrap_or(MacosInterruptionLevel::Passive),
            MacosInterruptionLevel::Active
        );
        assert_eq!(
            "time_sensitive"
                .parse::<MacosInterruptionLevel>()
                .unwrap_or(MacosInterruptionLevel::Active),
            MacosInterruptionLevel::TimeSensitive
        );
    }

    #[test]
    fn parse_priority_invalid() {
        assert!("critical".parse::<MacosInterruptionLevel>().is_err());
        assert!("".parse::<MacosInterruptionLevel>().is_err());
        assert!("ACTIVE".parse::<MacosInterruptionLevel>().is_err());
    }

    // ── MacosNotificationAction tests ───────────────────────────────────

    #[test]
    fn action_identifiers() {
        assert_eq!(MacosNotificationAction::Open.action_id(), "open");
        assert_eq!(MacosNotificationAction::Dismiss.action_id(), "dismiss");
    }

    #[test]
    fn action_button_titles() {
        assert_eq!(MacosNotificationAction::Open.button_title(), "Open");
        assert_eq!(MacosNotificationAction::Dismiss.button_title(), "Dismiss");
    }

    #[test]
    fn action_set_has_open_and_dismiss_in_order() {
        let actions = MacosNotificationAction::default_actions();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0], MacosNotificationAction::Open);
        assert_eq!(actions[1], MacosNotificationAction::Dismiss);
    }
}
