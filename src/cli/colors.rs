//! Terminal color theme with `NO_COLOR` support.

use owo_colors::OwoColorize;

/// Color theme for CLI output.
///
/// Respects the `NO_COLOR` environment variable. When `NO_COLOR` is set,
/// all formatting methods return plain unmodified text.
pub struct Theme {
    color_enabled: bool,
}

impl Theme {
    /// Detect terminal color support.
    ///
    /// Returns a theme with colors disabled if `NO_COLOR` is set.
    #[must_use]
    pub fn detect() -> Self {
        let color_enabled = std::env::var_os("NO_COLOR").is_none();
        Self { color_enabled }
    }

    /// Whether color output is enabled.
    #[must_use]
    pub fn color_enabled(&self) -> bool {
        self.color_enabled
    }

    /// Format the agent response prefix (e.g. "ironclaw:") in cyan.
    #[must_use]
    pub fn format_prefix(&self, label: &str) -> String {
        if self.color_enabled {
            format!("{}", label.cyan())
        } else {
            label.to_string()
        }
    }

    /// Format a tool name/info line in dim text.
    #[must_use]
    pub fn format_tool(&self, text: &str) -> String {
        if self.color_enabled {
            format!("{}", text.dimmed())
        } else {
            text.to_string()
        }
    }

    /// Format an error message in bold red.
    #[must_use]
    pub fn format_error(&self, text: &str) -> String {
        if self.color_enabled {
            let styled = text.red();
            format!("{}", styled.bold())
        } else {
            text.to_string()
        }
    }

    /// Format a system event line in yellow.
    #[must_use]
    pub fn format_system_event(&self, text: &str) -> String {
        if self.color_enabled {
            format!("{}", text.yellow())
        } else {
            text.to_string()
        }
    }

    /// Format the startup banner in bold cyan.
    #[must_use]
    pub fn format_banner(&self, text: &str) -> String {
        if self.color_enabled {
            let styled = text.cyan();
            format!("{}", styled.bold())
        } else {
            text.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_no_color_returns_plain_text() {
        let theme = Theme {
            color_enabled: false,
        };
        assert_eq!(
            theme.format_prefix("ironclaw:"),
            "ironclaw:",
            "plain theme should return unmodified text"
        );
        assert_eq!(
            theme.format_error("bad thing"),
            "bad thing",
            "plain theme should return unmodified error text"
        );
        assert_eq!(
            theme.format_tool("[tool: exec]"),
            "[tool: exec]",
            "plain theme should return unmodified tool text"
        );
        assert_eq!(
            theme.format_system_event("[cron] done"),
            "[cron] done",
            "plain theme should return unmodified event text"
        );
    }

    #[test]
    fn theme_color_enabled_returns_non_empty() {
        let theme = Theme {
            color_enabled: true,
        };
        let styled = theme.format_prefix("ironclaw:");
        assert!(
            !styled.is_empty(),
            "colored theme should return non-empty string"
        );
    }

    #[test]
    fn theme_detect_respects_field() {
        let theme = Theme {
            color_enabled: false,
        };
        assert!(
            !theme.color_enabled(),
            "color_enabled should reflect construction"
        );
    }
}
