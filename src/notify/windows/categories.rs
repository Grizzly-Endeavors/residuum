//! Notification categories, scenarios, and action definitions for Windows Toast.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Notification category (maps to conceptual grouping for throttle threading).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowsCategory {
    BackgroundResults,
    Reminders,
    InboxItems,
    Alerts,
}

impl WindowsCategory {
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

impl fmt::Display for WindowsCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_category_id())
    }
}

impl std::str::FromStr for WindowsCategory {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "background-results" => Ok(Self::BackgroundResults),
            "reminders" => Ok(Self::Reminders),
            "inbox-items" => Ok(Self::InboxItems),
            "alerts" => Ok(Self::Alerts),
            _ => anyhow::bail!("unknown Windows notification category: {s}"),
        }
    }
}

/// Maps to Windows Toast notification scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsScenario {
    /// Normal toast, auto-dismiss after timeout.
    Default,
    /// Stays on screen, must be manually dismissed.
    Reminder,
    /// Alarm-style, audio loops.
    Alarm,
}

impl WindowsScenario {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Reminder => "reminder",
            Self::Alarm => "alarm",
        }
    }
}

impl fmt::Display for WindowsScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for WindowsScenario {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "default" => Ok(Self::Default),
            "reminder" => Ok(Self::Reminder),
            "alarm" => Ok(Self::Alarm),
            _ => anyhow::bail!("unknown Windows notification scenario: {s}"),
        }
    }
}

/// Action buttons for Windows Toast notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsNotificationAction {
    Open,
    Dismiss,
}

impl WindowsNotificationAction {
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
    pub fn for_category() -> &'static [Self] {
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

    // -- WindowsCategory tests -----------------------------------------------

    #[test]
    fn category_serde_roundtrip() {
        for cat in WindowsCategory::all() {
            let json =
                serde_json::to_string(cat).unwrap_or_else(|_| String::from("serialize failed"));
            let parsed: WindowsCategory =
                serde_json::from_str(&json).unwrap_or(WindowsCategory::BackgroundResults);
            assert_eq!(*cat, parsed, "roundtrip failed for {cat}");
        }
    }

    #[test]
    fn category_kebab_case_serialization() {
        assert_eq!(
            serde_json::to_string(&WindowsCategory::BackgroundResults).unwrap_or_default(),
            "\"background-results\""
        );
        assert_eq!(
            serde_json::to_string(&WindowsCategory::InboxItems).unwrap_or_default(),
            "\"inbox-items\""
        );
    }

    #[test]
    fn category_display() {
        assert_eq!(
            WindowsCategory::BackgroundResults.to_string(),
            "background-results"
        );
        assert_eq!(WindowsCategory::Reminders.to_string(), "reminders");
        assert_eq!(WindowsCategory::InboxItems.to_string(), "inbox-items");
        assert_eq!(WindowsCategory::Alerts.to_string(), "alerts");
    }

    #[test]
    fn category_id_matches_display() {
        for cat in WindowsCategory::all() {
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
            "background-results"
                .parse::<WindowsCategory>()
                .unwrap_or(WindowsCategory::Alerts),
            WindowsCategory::BackgroundResults
        );
        assert_eq!(
            "reminders"
                .parse::<WindowsCategory>()
                .unwrap_or(WindowsCategory::Alerts),
            WindowsCategory::Reminders
        );
        assert_eq!(
            "inbox-items"
                .parse::<WindowsCategory>()
                .unwrap_or(WindowsCategory::Alerts),
            WindowsCategory::InboxItems
        );
        assert_eq!(
            "alerts"
                .parse::<WindowsCategory>()
                .unwrap_or(WindowsCategory::BackgroundResults),
            WindowsCategory::Alerts
        );
    }

    #[test]
    fn parse_category_invalid() {
        assert!("unknown".parse::<WindowsCategory>().is_err());
        assert!("".parse::<WindowsCategory>().is_err());
        assert!("ALERTS".parse::<WindowsCategory>().is_err());
    }

    // -- WindowsScenario tests -----------------------------------------------

    #[test]
    fn scenario_serde_roundtrip() {
        let scenarios = [
            WindowsScenario::Default,
            WindowsScenario::Reminder,
            WindowsScenario::Alarm,
        ];
        for scenario in &scenarios {
            let json = serde_json::to_string(scenario)
                .unwrap_or_else(|_| String::from("serialize failed"));
            let parsed: WindowsScenario =
                serde_json::from_str(&json).unwrap_or(WindowsScenario::Default);
            assert_eq!(*scenario, parsed, "roundtrip failed for {scenario}");
        }
    }

    #[test]
    fn scenario_snake_case_serialization() {
        assert_eq!(
            serde_json::to_string(&WindowsScenario::Default).unwrap_or_default(),
            "\"default\""
        );
        assert_eq!(
            serde_json::to_string(&WindowsScenario::Reminder).unwrap_or_default(),
            "\"reminder\""
        );
        assert_eq!(
            serde_json::to_string(&WindowsScenario::Alarm).unwrap_or_default(),
            "\"alarm\""
        );
    }

    #[test]
    fn scenario_display() {
        assert_eq!(WindowsScenario::Default.to_string(), "default");
        assert_eq!(WindowsScenario::Reminder.to_string(), "reminder");
        assert_eq!(WindowsScenario::Alarm.to_string(), "alarm");
    }

    #[test]
    fn parse_scenario_valid() {
        assert_eq!(
            "default"
                .parse::<WindowsScenario>()
                .unwrap_or(WindowsScenario::Alarm),
            WindowsScenario::Default
        );
        assert_eq!(
            "reminder"
                .parse::<WindowsScenario>()
                .unwrap_or(WindowsScenario::Default),
            WindowsScenario::Reminder
        );
        assert_eq!(
            "alarm"
                .parse::<WindowsScenario>()
                .unwrap_or(WindowsScenario::Default),
            WindowsScenario::Alarm
        );
    }

    #[test]
    fn parse_scenario_invalid() {
        assert!("critical".parse::<WindowsScenario>().is_err());
        assert!("".parse::<WindowsScenario>().is_err());
        assert!("DEFAULT".parse::<WindowsScenario>().is_err());
    }

    // -- WindowsNotificationAction tests -------------------------------------

    #[test]
    fn action_identifiers() {
        assert_eq!(WindowsNotificationAction::Open.action_id(), "open");
        assert_eq!(WindowsNotificationAction::Dismiss.action_id(), "dismiss");
    }

    #[test]
    fn action_button_titles() {
        assert_eq!(WindowsNotificationAction::Open.button_title(), "Open");
        assert_eq!(WindowsNotificationAction::Dismiss.button_title(), "Dismiss");
    }

    #[test]
    fn action_set_has_open_and_dismiss_in_order() {
        let actions = WindowsNotificationAction::for_category();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0], WindowsNotificationAction::Open);
        assert_eq!(actions[1], WindowsNotificationAction::Dismiss);
    }
}
