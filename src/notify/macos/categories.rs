//! Notification categories, interruption levels, and action definitions.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::bus::EventTrigger;

/// Notification category mapped to macOS `UNNotificationCategory` identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MacosCategory {
    BackgroundResults,
    Reminders,
    InboxItems,
    Alerts,
}

impl MacosCategory {
    #[must_use]
    pub fn as_category_id(&self) -> &'static str {
        match self {
            Self::BackgroundResults => "background-results",
            Self::Reminders => "reminders",
            Self::InboxItems => "inbox-items",
            Self::Alerts => "alerts",
        }
    }

    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::BackgroundResults,
            Self::Reminders,
            Self::InboxItems,
            Self::Alerts,
        ]
    }
}

impl fmt::Display for MacosCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_category_id())
    }
}

/// # Errors
/// Returns an error if the string does not match any known category.
pub fn parse_category(s: &str) -> anyhow::Result<MacosCategory> {
    match s {
        "background-results" => Ok(MacosCategory::BackgroundResults),
        "reminders" => Ok(MacosCategory::Reminders),
        "inbox-items" => Ok(MacosCategory::InboxItems),
        "alerts" => Ok(MacosCategory::Alerts),
        _ => anyhow::bail!("unknown macOS notification category: {s}"),
    }
}

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

/// # Errors
/// Returns an error if the string does not match any known priority.
pub fn parse_priority(s: &str) -> anyhow::Result<MacosInterruptionLevel> {
    match s {
        "passive" => Ok(MacosInterruptionLevel::Passive),
        "active" => Ok(MacosInterruptionLevel::Active),
        "time_sensitive" => Ok(MacosInterruptionLevel::TimeSensitive),
        _ => anyhow::bail!("unknown macOS notification priority: {s}"),
    }
}

#[must_use]
pub fn default_category_for_source(source: &EventTrigger) -> MacosCategory {
    match source {
        EventTrigger::Pulse | EventTrigger::Agent => MacosCategory::BackgroundResults,
        EventTrigger::Action => MacosCategory::Reminders,
        EventTrigger::Webhook(_) => MacosCategory::BackgroundResults,
    }
}

#[must_use]
pub fn resolve_category(_source: &EventTrigger, channel_default: MacosCategory) -> MacosCategory {
    // Channel config default takes precedence over source-based mapping
    // when explicitly configured. Since we always have a default, use it.
    channel_default
}

/// Action buttons displayed on notification banners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosNotificationAction {
    Open,
    Dismiss,
    MarkRead,
}

impl MacosNotificationAction {
    #[must_use]
    pub fn action_id(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Dismiss => "dismiss",
            Self::MarkRead => "mark-read",
        }
    }

    #[must_use]
    pub fn button_title(&self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Dismiss => "Dismiss",
            Self::MarkRead => "Mark Read",
        }
    }

    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::Open, Self::Dismiss, Self::MarkRead]
    }

    #[must_use]
    pub fn for_category(category: MacosCategory) -> &'static [Self] {
        match category {
            MacosCategory::BackgroundResults | MacosCategory::Reminders | MacosCategory::Alerts => {
                &[Self::Open, Self::Dismiss]
            }
            MacosCategory::InboxItems => &[Self::Open, Self::MarkRead],
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

    // ── MacosCategory tests ─────────────────────────────────────────────

    #[test]
    fn category_serde_roundtrip() {
        for cat in MacosCategory::all() {
            let json =
                serde_json::to_string(cat).unwrap_or_else(|_| String::from("serialize failed"));
            let parsed: MacosCategory =
                serde_json::from_str(&json).unwrap_or(MacosCategory::BackgroundResults);
            assert_eq!(*cat, parsed, "roundtrip failed for {cat}");
        }
    }

    #[test]
    fn category_kebab_case_serialization() {
        assert_eq!(
            serde_json::to_string(&MacosCategory::BackgroundResults).unwrap_or_default(),
            "\"background-results\""
        );
        assert_eq!(
            serde_json::to_string(&MacosCategory::InboxItems).unwrap_or_default(),
            "\"inbox-items\""
        );
    }

    #[test]
    fn category_display() {
        assert_eq!(
            MacosCategory::BackgroundResults.to_string(),
            "background-results"
        );
        assert_eq!(MacosCategory::Reminders.to_string(), "reminders");
        assert_eq!(MacosCategory::InboxItems.to_string(), "inbox-items");
        assert_eq!(MacosCategory::Alerts.to_string(), "alerts");
    }

    #[test]
    fn category_id_matches_display() {
        for cat in MacosCategory::all() {
            assert_eq!(
                cat.as_category_id(),
                cat.to_string(),
                "category_id and display should match for {cat}"
            );
        }
    }

    #[test]
    fn parse_category_valid() {
        assert_eq!(
            parse_category("background-results").unwrap_or(MacosCategory::Alerts),
            MacosCategory::BackgroundResults
        );
        assert_eq!(
            parse_category("reminders").unwrap_or(MacosCategory::Alerts),
            MacosCategory::Reminders
        );
        assert_eq!(
            parse_category("inbox-items").unwrap_or(MacosCategory::Alerts),
            MacosCategory::InboxItems
        );
        assert_eq!(
            parse_category("alerts").unwrap_or(MacosCategory::BackgroundResults),
            MacosCategory::Alerts
        );
    }

    #[test]
    fn parse_category_invalid() {
        assert!(parse_category("unknown").is_err());
        assert!(parse_category("").is_err());
        assert!(parse_category("ALERTS").is_err());
    }

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
            parse_priority("passive").unwrap_or(MacosInterruptionLevel::Active),
            MacosInterruptionLevel::Passive
        );
        assert_eq!(
            parse_priority("active").unwrap_or(MacosInterruptionLevel::Passive),
            MacosInterruptionLevel::Active
        );
        assert_eq!(
            parse_priority("time_sensitive").unwrap_or(MacosInterruptionLevel::Active),
            MacosInterruptionLevel::TimeSensitive
        );
    }

    #[test]
    fn parse_priority_invalid() {
        assert!(parse_priority("critical").is_err());
        assert!(parse_priority("").is_err());
        assert!(parse_priority("ACTIVE").is_err());
    }

    // ── EventTrigger-to-Category mapping tests ───────────────────────────

    #[test]
    fn default_category_for_source_mapping() {
        assert_eq!(
            default_category_for_source(&EventTrigger::Pulse),
            MacosCategory::BackgroundResults
        );
        assert_eq!(
            default_category_for_source(&EventTrigger::Action),
            MacosCategory::Reminders
        );
        assert_eq!(
            default_category_for_source(&EventTrigger::Agent),
            MacosCategory::BackgroundResults
        );
        assert_eq!(
            default_category_for_source(&EventTrigger::Webhook("github".to_string())),
            MacosCategory::BackgroundResults
        );
    }

    // ── MacosNotificationAction tests ───────────────────────────────────

    #[test]
    fn action_variant_count() {
        assert_eq!(
            MacosNotificationAction::all().len(),
            3,
            "should have exactly 3 action variants"
        );
    }

    #[test]
    fn action_identifiers() {
        assert_eq!(MacosNotificationAction::Open.action_id(), "open");
        assert_eq!(MacosNotificationAction::Dismiss.action_id(), "dismiss");
        assert_eq!(MacosNotificationAction::MarkRead.action_id(), "mark-read");
    }

    #[test]
    fn action_button_titles() {
        assert_eq!(MacosNotificationAction::Open.button_title(), "Open");
        assert_eq!(MacosNotificationAction::Dismiss.button_title(), "Dismiss");
        assert_eq!(
            MacosNotificationAction::MarkRead.button_title(),
            "Mark Read"
        );
    }

    #[test]
    fn actions_for_background_results() {
        let actions = MacosNotificationAction::for_category(MacosCategory::BackgroundResults);
        assert_eq!(actions.len(), 2, "BackgroundResults should have 2 actions");
        assert_eq!(actions[0], MacosNotificationAction::Open);
        assert_eq!(actions[1], MacosNotificationAction::Dismiss);
    }

    #[test]
    fn actions_for_inbox_items() {
        let actions = MacosNotificationAction::for_category(MacosCategory::InboxItems);
        assert_eq!(actions.len(), 2, "InboxItems should have 2 actions");
        assert_eq!(actions[0], MacosNotificationAction::Open);
        assert_eq!(actions[1], MacosNotificationAction::MarkRead);
    }

    #[test]
    fn actions_for_reminders() {
        let actions = MacosNotificationAction::for_category(MacosCategory::Reminders);
        assert_eq!(actions.len(), 2, "Reminders should have 2 actions");
        assert_eq!(actions[0], MacosNotificationAction::Open);
        assert_eq!(actions[1], MacosNotificationAction::Dismiss);
    }

    #[test]
    fn actions_for_alerts() {
        let actions = MacosNotificationAction::for_category(MacosCategory::Alerts);
        assert_eq!(actions.len(), 2, "Alerts should have 2 actions");
        assert_eq!(actions[0], MacosNotificationAction::Open);
        assert_eq!(actions[1], MacosNotificationAction::Dismiss);
    }
}
