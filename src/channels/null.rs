//! No-op display for background turns (pulse, cron).

use super::TurnDisplay;

/// A display implementation that discards all output.
///
/// Used for background pulse and cron turns where no user is watching.
pub struct NullDisplay;

impl TurnDisplay for NullDisplay {
    fn show_tool_call(&self, _name: &str, _args: &serde_json::Value) {}

    fn show_tool_result(&self, _name: &str, _output: &str, _is_error: bool) {}

    fn show_response(&self, _content: &str) {}
}
