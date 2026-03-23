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

    /// Format the agent response prefix (e.g. "residuum:") in cyan.
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

    /// Format the user input prompt with bold styling.
    ///
    /// Uses raw ANSI escapes which rustyline handles natively for width calculation.
    #[must_use]
    pub fn format_user_prompt(&self) -> String {
        if self.color_enabled {
            format!("{bold}You: {reset}", bold = "\x1b[1;36m", reset = "\x1b[0m",)
        } else {
            "You: ".to_string()
        }
    }

    /// Format a memory operation notice in bright green.
    #[must_use]
    pub fn format_notice(&self, text: &str) -> String {
        if self.color_enabled {
            format!("{}", text.bright_green())
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
            theme.format_prefix("Residuum:"),
            "Residuum:",
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
        let styled = theme.format_prefix("Residuum:");
        assert!(
            styled.contains("\x1b["),
            "colored theme should contain ANSI escape codes"
        );
    }

    #[test]
    fn format_user_prompt_no_color() {
        let theme = Theme {
            color_enabled: false,
        };
        assert_eq!(
            theme.format_user_prompt(),
            "You: ",
            "plain theme should return plain prompt"
        );
    }

    #[test]
    fn format_user_prompt_with_color() {
        let theme = Theme {
            color_enabled: true,
        };
        let prompt = theme.format_user_prompt();
        assert!(
            prompt.contains("You: "),
            "colored prompt should contain 'You: '"
        );
        assert!(
            prompt.contains("\x1b[1;36m"),
            "colored prompt should have ANSI bold cyan escape"
        );
        assert!(
            !prompt.contains('\x01'),
            "colored prompt should not have readline escape markers"
        );
    }

    #[test]
    fn theme_notice_no_color_passthrough() {
        let theme = Theme {
            color_enabled: false,
        };
        assert_eq!(
            theme.format_notice("memory op done"),
            "memory op done",
            "plain theme should return unmodified notice text"
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
