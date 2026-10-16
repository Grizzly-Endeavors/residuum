//! Cumulative token usage of a whole session, carried on the bus so the web
//! UI can show a conversation's size.
//!
//! The agent accumulates these totals and persists them; the bus only carries
//! the snapshot. See `docs/systems-usage/turn-control.md`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::inference::Usage;

/// Cumulative token totals for a whole session (the main chat, or one
/// background session run), persisted so a reload shows correct totals
/// rather than resetting to zero. See `docs/systems-usage/turn-control.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionUsageTotals {
    /// Prompt tokens sent across every model call this session has made.
    #[ts(type = "number")]
    pub input_tokens: u64,
    /// Completion tokens generated across every model call this session
    /// has made.
    #[ts(type = "number")]
    pub output_tokens: u64,
    /// Context size at the most recent model call: that call's input
    /// tokens plus its cache reads/writes — the actual token count the
    /// provider processed as context, not an estimate. `None` until the
    /// first call that reports usage. There is no per-model context-window
    /// size known to the codebase, so this is shown as an absolute count
    /// rather than a percentage.
    pub context_tokens: Option<u32>,
    /// Tool calls executed across every turn this session has run. Every
    /// tool call an executed batch carries counts individually, including
    /// calls made in parallel within the same model response.
    ///
    /// `#[serde(default)]` so a totals file or run record persisted before
    /// this field existed still deserializes, showing zero rather than
    /// failing to load.
    #[serde(default)]
    #[ts(type = "number")]
    pub tool_calls: u64,
}

impl SessionUsageTotals {
    /// Fold one model call's usage into these running totals. A `None`
    /// usage (a provider that didn't report any) leaves them unchanged.
    pub fn accumulate(&mut self, usage: Option<Usage>) {
        let Some(usage) = usage else {
            return;
        };
        self.input_tokens += u64::from(usage.input_tokens);
        self.output_tokens += u64::from(usage.output_tokens);
        self.context_tokens = Some(
            usage.input_tokens
                + usage.cache_creation_tokens.unwrap_or(0)
                + usage.cache_read_tokens.unwrap_or(0),
        );
    }

    /// Fold `n` more executed tool calls into these running totals.
    pub fn record_tool_calls(&mut self, n: u32) {
        self.tool_calls += u64::from(n);
    }
}
