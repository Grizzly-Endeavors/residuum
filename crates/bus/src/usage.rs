//! Cumulative token usage of a whole session, carried on the bus so the web
//! UI can show a conversation's size.
//!
//! The agent accumulates these totals and persists them; the bus only carries
//! the snapshot. See `docs/systems-usage/turn-control.md`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use residuum_inference::Usage;

/// Cumulative token totals for a whole session (the main chat, or one
/// background session run), persisted so a reload shows correct totals
/// rather than resetting to zero. See `docs/systems-usage/turn-control.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionUsageTotals {
    /// Input tokens sent across every model call this session has made,
    /// counting every prompt whole: uncached input, cache reads and cache
    /// writes together ([`Usage::total_input_tokens`]). The uncached part is
    /// this minus `cache_read_tokens` and `cache_write_tokens`.
    #[ts(type = "number")]
    pub input_tokens: u64,
    /// The part of `input_tokens` read from the prompt cache.
    ///
    /// `#[serde(default)]` so a totals file or run record persisted before
    /// this field existed still deserializes, showing zero.
    #[serde(default)]
    #[ts(type = "number")]
    pub cache_read_tokens: u64,
    /// The part of `input_tokens` written to the prompt cache.
    ///
    /// `#[serde(default)]` for the same reason as `cache_read_tokens`.
    #[serde(default)]
    #[ts(type = "number")]
    pub cache_write_tokens: u64,
    /// Output tokens generated across every model call this session has
    /// made, reasoning included.
    #[ts(type = "number")]
    pub output_tokens: u64,
    /// Context size at the most recent model call that reported its input:
    /// that call's whole prompt, cached or not — the token count the
    /// provider processed as context, not an estimate. `None` until the
    /// first call that reports input. There is no per-model context-window
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
    /// Fold one model call's usage into these running totals. A count the
    /// provider didn't report adds nothing, and a `None` usage leaves the
    /// totals unchanged.
    pub fn accumulate(&mut self, usage: Option<Usage>) {
        let Some(usage) = usage else {
            return;
        };
        let total_input = usage.total_input_tokens();
        self.input_tokens += u64::from(total_input.unwrap_or(0));
        self.cache_read_tokens += u64::from(usage.cache_read_tokens.unwrap_or(0));
        self.cache_write_tokens += u64::from(usage.cache_write_tokens.unwrap_or(0));
        self.output_tokens += u64::from(usage.output_tokens.unwrap_or(0));
        if total_input.is_some() {
            self.context_tokens = total_input;
        }
    }

    /// Fold `n` more executed tool calls into these running totals.
    pub fn record_tool_calls(&mut self, n: u32) {
        self.tool_calls += u64::from(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            uncached_input_tokens: Some(input),
            output_tokens: Some(output),
            ..Usage::default()
        }
    }

    #[test]
    fn session_usage_totals_accumulate_input_and_output() {
        let mut totals = SessionUsageTotals::default();
        totals.accumulate(Some(usage(100, 20)));
        totals.accumulate(Some(usage(150, 30)));
        assert_eq!(totals.input_tokens, 250);
        assert_eq!(totals.output_tokens, 50);
    }

    #[test]
    fn session_usage_totals_context_tokens_is_the_latest_calls_full_context() {
        let mut totals = SessionUsageTotals::default();
        totals.accumulate(Some(Usage {
            uncached_input_tokens: Some(100),
            cache_read_tokens: Some(5),
            cache_write_tokens: Some(10),
            output_tokens: Some(20),
            ..Usage::default()
        }));
        assert_eq!(totals.context_tokens, Some(115));
        totals.accumulate(Some(usage(40, 10)));
        assert_eq!(
            totals.context_tokens,
            Some(40),
            "context size reflects only the most recent call, not a running sum"
        );
    }

    /// The same 10,000-token prompt with 9,000 tokens read from the cache,
    /// as an OpenAI-shaped and an Anthropic-shaped provider report it once
    /// normalized: `OpenAI`'s `prompt_tokens` counted the cache, Anthropic's
    /// `input_tokens` did not.
    #[test]
    fn session_usage_totals_count_a_prompt_the_same_for_every_provider_shape() {
        let openai_shaped = Usage {
            uncached_input_tokens: Some(1_000),
            cache_read_tokens: Some(9_000),
            cache_write_tokens: None,
            output_tokens: Some(50),
            ..Usage::default()
        };
        let anthropic_shaped = Usage {
            uncached_input_tokens: Some(1_000),
            cache_read_tokens: Some(9_000),
            cache_write_tokens: Some(0),
            output_tokens: Some(50),
            ..Usage::default()
        };
        let mut openai = SessionUsageTotals::default();
        openai.accumulate(Some(openai_shaped));
        let mut anthropic = SessionUsageTotals::default();
        anthropic.accumulate(Some(anthropic_shaped));
        assert_eq!(openai.context_tokens, Some(10_000));
        assert_eq!(
            openai, anthropic,
            "a cache hit is counted once, whichever way the provider reported it"
        );
        assert_eq!(openai.input_tokens, 10_000);
        assert_eq!(openai.cache_read_tokens, 9_000);
    }

    #[test]
    fn session_usage_totals_keep_the_last_known_context_when_a_call_reports_no_input() {
        let mut totals = SessionUsageTotals::default();
        totals.accumulate(Some(usage(100, 20)));
        totals.accumulate(Some(Usage {
            output_tokens: Some(7),
            ..Usage::default()
        }));
        assert_eq!(
            totals.context_tokens,
            Some(100),
            "an unknown input count is not a context of zero"
        );
        assert_eq!(totals.input_tokens, 100, "an unknown count adds nothing");
        assert_eq!(totals.output_tokens, 27);
    }

    #[test]
    fn session_usage_totals_accumulate_cache_reads_and_writes() {
        let mut totals = SessionUsageTotals::default();
        let call = Usage {
            uncached_input_tokens: Some(10),
            cache_read_tokens: Some(300),
            cache_write_tokens: Some(40),
            output_tokens: Some(5),
            ..Usage::default()
        };
        totals.accumulate(Some(call));
        totals.accumulate(Some(call));
        assert_eq!(totals.cache_read_tokens, 600);
        assert_eq!(totals.cache_write_tokens, 80);
        assert_eq!(totals.input_tokens, 700);
    }

    #[test]
    fn session_usage_totals_none_leaves_everything_unchanged() {
        let mut totals = SessionUsageTotals::default();
        totals.accumulate(Some(usage(100, 20)));
        totals.accumulate(None);
        assert_eq!(totals.input_tokens, 100);
        assert_eq!(totals.output_tokens, 20);
        assert_eq!(totals.context_tokens, Some(100));
    }

    #[test]
    fn session_usage_totals_record_tool_calls_accumulates() {
        let mut totals = SessionUsageTotals::default();
        totals.record_tool_calls(4);
        totals.record_tool_calls(6);
        assert_eq!(totals.tool_calls, 10);
    }

    #[test]
    fn session_usage_totals_deserializes_a_payload_persisted_before_tool_calls_existed() {
        let old_payload = serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 20,
            "context_tokens": 100,
        });
        let totals: SessionUsageTotals = serde_json::from_value(old_payload).unwrap();
        assert_eq!(totals.input_tokens, 100);
        assert_eq!(totals.output_tokens, 20);
        assert_eq!(totals.context_tokens, Some(100));
        assert_eq!(
            totals.tool_calls, 0,
            "a payload persisted before tool_calls existed should default to zero, not fail to parse"
        );
        assert_eq!(
            (totals.cache_read_tokens, totals.cache_write_tokens),
            (0, 0),
            "a payload persisted before the cache totals existed loads with them at zero"
        );
    }
}
