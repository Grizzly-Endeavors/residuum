//! Estimated spend per model call: the built-in price table, the `[pricing]`
//! overrides from `hub/config.toml`, and the arithmetic that prices one call's
//! [`Usage`]. The result is an estimate; the provider's own bill is the record
//! of what was charged.

use std::collections::BTreeMap;

use serde::Serialize;

use residuum_config::ModelPrice;

use crate::Usage;

/// Built-in prices, keyed `provider/model` with the provider kind as usage
/// records name it. Each is the base tier of the provider's published price
/// list. A provider that bills longer prompts at a higher rate is priced at
/// its base tier, so an estimate for those prompts runs low until an override
/// states the rate that applies.
const BUILT_IN: &[(&str, ModelPrice)] = &[
    (
        "anthropic/claude-opus-5-5",
        ModelPrice {
            input: 4.0,
            output: 20.0,
            cache_read: 0.20,
            cache_write: 5.0,
            cache_write_long: 8.0,
        },
    ),
    (
        "anthropic/claude-sonnet-5-5",
        ModelPrice {
            input: 2.0,
            output: 10.0,
            cache_read: 0.10,
            cache_write: 2.50,
            cache_write_long: 4.0,
        },
    ),
    (
        "anthropic/claude-haiku-5-5",
        ModelPrice {
            input: 0.10,
            output: 0.50,
            cache_read: 0.01,
            cache_write: 0.125,
            cache_write_long: 0.20,
        },
    ),
    (
        "anthropic/claude-opus-4-6",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_read: 0.50,
            cache_write: 6.25,
            cache_write_long: 10.0,
        },
    ),
    (
        "anthropic/claude-sonnet-4-6",
        ModelPrice {
            input: 3.0,
            output: 15.0,
            cache_read: 0.30,
            cache_write: 3.75,
            cache_write_long: 6.0,
        },
    ),
    (
        "anthropic/claude-haiku-4-5",
        ModelPrice {
            input: 1.0,
            output: 5.0,
            cache_read: 0.10,
            cache_write: 1.25,
            cache_write_long: 2.0,
        },
    ),
    (
        "openai/gpt-4o",
        ModelPrice {
            input: 2.50,
            output: 10.0,
            cache_read: 1.25,
            cache_write: 2.50,
            cache_write_long: 2.50,
        },
    ),
    (
        "openai/gpt-4o-mini",
        ModelPrice {
            input: 0.15,
            output: 0.60,
            cache_read: 0.075,
            cache_write: 0.15,
            cache_write_long: 0.15,
        },
    ),
    (
        "openai/o3",
        ModelPrice {
            input: 2.00,
            output: 8.00,
            cache_read: 0.50,
            cache_write: 2.00,
            cache_write_long: 2.00,
        },
    ),
    (
        "openai/gpt-5",
        ModelPrice {
            input: 1.25,
            output: 10.0,
            cache_read: 0.125,
            cache_write: 1.25,
            cache_write_long: 1.25,
        },
    ),
    (
        "openai/gpt-5-mini",
        ModelPrice {
            input: 0.25,
            output: 2.00,
            cache_read: 0.025,
            cache_write: 0.25,
            cache_write_long: 0.25,
        },
    ),
    (
        "gemini/gemini-2.5-pro",
        ModelPrice {
            input: 1.25,
            output: 10.0,
            cache_read: 0.125,
            cache_write: 1.25,
            cache_write_long: 1.25,
        },
    ),
    (
        "gemini/gemini-2.5-flash",
        ModelPrice {
            input: 0.30,
            output: 2.50,
            cache_read: 0.03,
            cache_write: 0.30,
            cache_write_long: 0.30,
        },
    ),
    (
        "gemini/gemini-3.8-flash",
        ModelPrice {
            input: 0.75,
            output: 3.75,
            cache_read: 0.075,
            cache_write: 0.75,
            cache_write_long: 0.75,
        },
    ),
    (
        "gemini/gemini-3.6-flash",
        ModelPrice {
            input: 0.75,
            output: 3.75,
            cache_read: 0.075,
            cache_write: 0.75,
            cache_write_long: 0.75,
        },
    ),
    (
        "gemini/gemini-3-flash-preview",
        ModelPrice {
            input: 0.50,
            output: 3.00,
            cache_read: 0.05,
            cache_write: 0.50,
            cache_write_long: 0.50,
        },
    ),
    (
        "gemini/gemini-3.1-pro-preview",
        ModelPrice {
            input: 2.00,
            output: 12.00,
            cache_read: 0.20,
            cache_write: 2.00,
            cache_write_long: 2.00,
        },
    ),
    (
        "fireworks/accounts/fireworks/models/kimi-k3",
        ModelPrice {
            input: 3.0,
            output: 15.0,
            cache_read: 0.30,
            cache_write: 3.0,
            cache_write_long: 3.0,
        },
    ),
    (
        "fireworks/accounts/fireworks/models/glm-5p3",
        ModelPrice {
            input: 1.40,
            output: 4.40,
            cache_read: 0.26,
            cache_write: 1.40,
            cache_write_long: 1.40,
        },
    ),
    (
        "fireworks/accounts/fireworks/models/glm-5p3-flash",
        ModelPrice {
            input: 0.15,
            output: 0.50,
            cache_read: 0.03,
            cache_write: 0.15,
            cache_write_long: 0.15,
        },
    ),
];

/// What one model call cost, in US dollars.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CostEstimate {
    /// Estimated spend in US dollars.
    pub usd: f64,
}

/// Prices by `provider/model`: the built-in table with the config's overrides
/// applied.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceTable {
    prices: BTreeMap<String, ModelPrice>,
}

impl PriceTable {
    /// The built-in table, with each entry in `overrides` replacing the one
    /// for its model.
    #[must_use]
    pub fn new(overrides: &BTreeMap<String, ModelPrice>) -> Self {
        let mut prices: BTreeMap<String, ModelPrice> = BUILT_IN
            .iter()
            .map(|(key, price)| ((*key).to_string(), *price))
            .collect();
        prices.extend(overrides.iter().map(|(key, price)| (key.clone(), *price)));
        Self { prices }
    }

    /// The estimated cost of one call to `model` at `provider` that reported
    /// `usage`, or `None` when the model has no price. An exact match wins;
    /// otherwise a trailing `-YYYYMMDD` date is dropped and the base model is
    /// matched.
    #[must_use]
    pub fn estimate(&self, provider: &str, model: &str, usage: &Usage) -> Option<CostEstimate> {
        let price = self.price(provider, model)?;
        let cache_write_long = usage.cache_write_long_tokens.unwrap_or(0);
        // The rest of the cache writes used the default, 5-minute TTL.
        let cache_write_short = usage
            .cache_write_tokens
            .unwrap_or(0)
            .saturating_sub(cache_write_long);
        // Reasoning tokens are already counted in the output tokens, so they
        // are not priced a second time.
        let usd = cost(usage.uncached_input_tokens.unwrap_or(0), price.input)
            + cost(usage.cache_read_tokens.unwrap_or(0), price.cache_read)
            + cost(cache_write_short, price.cache_write)
            + cost(cache_write_long, price.cache_write_long)
            + cost(usage.output_tokens.unwrap_or(0), price.output);
        Some(CostEstimate { usd })
    }

    fn price(&self, provider: &str, model: &str) -> Option<ModelPrice> {
        self.prices
            .get(&table_key(provider, model))
            .or_else(|| {
                let base = strip_date_suffix(model)?;
                self.prices.get(&table_key(provider, base))
            })
            .copied()
    }
}

/// The USD for `tokens` at `usd_per_million` dollars per million tokens.
fn cost(tokens: u32, usd_per_million: f64) -> f64 {
    f64::from(tokens) * usd_per_million / 1_000_000.0
}

fn table_key(provider: &str, model: &str) -> String {
    format!("{provider}/{model}")
}

/// `model` without a trailing `-YYYYMMDD` date, the suffix dated snapshots
/// carry (`claude-haiku-4-5-20251001`).
fn strip_date_suffix(model: &str) -> Option<&str> {
    let (base, date) = model.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit())).then_some(base)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use residuum_config::ModelSpec;

    use super::*;

    const EPSILON: f64 = 1e-9;

    fn assert_usd(estimate: Option<CostEstimate>, expected: f64) {
        let usd = estimate.expect("the model has a price").usd;
        assert!(
            (usd - expected).abs() < EPSILON,
            "expected {expected} USD, got {usd}"
        );
    }

    fn million(field: fn(&mut Usage, u32)) -> Usage {
        let mut usage = Usage::default();
        field(&mut usage, 1_000_000);
        usage
    }

    #[test]
    fn every_field_is_priced_at_its_own_rate() {
        // claude-sonnet-4-6: $3 input, $0.30 read, $3.75 5-minute write, $6
        // 1-hour write, $15 output, per million tokens.
        let usage = Usage {
            uncached_input_tokens: Some(1_000_000),
            cache_read_tokens: Some(1_000_000),
            cache_write_tokens: Some(2_000_000),
            cache_write_long_tokens: Some(1_000_000),
            output_tokens: Some(1_000_000),
            reasoning_tokens: None,
        };

        let table = PriceTable::new(&BTreeMap::new());

        // 3 + 0.30 + (1M at 3.75) + (1M at 6) + 15
        assert_usd(
            table.estimate("anthropic", "claude-sonnet-4-6", &usage),
            28.05,
        );
    }

    #[test]
    fn a_small_count_is_priced_proportionally() {
        let usage = Usage {
            uncached_input_tokens: Some(1234),
            ..Usage::default()
        };

        let table = PriceTable::new(&BTreeMap::new());

        assert_usd(table.estimate("openai", "gpt-4o", &usage), 0.003_085);
    }

    #[test]
    fn a_cache_write_is_priced_at_the_input_rate_when_the_provider_has_no_write_price() {
        let usage = Usage {
            cache_write_tokens: Some(1_000_000),
            ..Usage::default()
        };

        let table = PriceTable::new(&BTreeMap::new());

        assert_usd(table.estimate("openai", "gpt-4o", &usage), 2.5);
    }

    #[test]
    fn unreported_fields_count_as_zero() {
        let table = PriceTable::new(&BTreeMap::new());

        assert_usd(
            table.estimate("anthropic", "claude-sonnet-4-6", &Usage::default()),
            0.0,
        );
        assert_usd(
            table.estimate(
                "anthropic",
                "claude-sonnet-4-6",
                &million(|usage, n| usage.output_tokens = Some(n)),
            ),
            15.0,
        );
    }

    #[test]
    fn reasoning_tokens_are_not_priced_a_second_time() {
        let usage = Usage {
            output_tokens: Some(1_000_000),
            reasoning_tokens: Some(400_000),
            ..Usage::default()
        };

        let table = PriceTable::new(&BTreeMap::new());

        assert_usd(
            table.estimate("anthropic", "claude-sonnet-4-6", &usage),
            15.0,
        );
    }

    #[test]
    fn an_unpriced_model_has_no_estimate() {
        let table = PriceTable::new(&BTreeMap::new());
        let usage = million(|usage, n| usage.uncached_input_tokens = Some(n));

        assert_eq!(table.estimate("ollama", "llama3", &usage), None);
        assert_eq!(table.estimate("openai", "gpt-9", &usage), None);
        assert_eq!(table.estimate("anthropic", "claude-sonnet-4", &usage), None);
    }

    #[test]
    fn an_override_replaces_the_built_in_price() {
        let overrides = BTreeMap::from([(
            "openai/gpt-4o".to_string(),
            ModelPrice {
                input: 1.0,
                output: 2.0,
                cache_read: 0.1,
                cache_write: 1.0,
                cache_write_long: 1.0,
            },
        )]);
        let usage = million(|usage, n| usage.uncached_input_tokens = Some(n));

        let table = PriceTable::new(&overrides);

        assert_usd(table.estimate("openai", "gpt-4o", &usage), 1.0);
    }

    #[test]
    fn an_override_can_price_a_model_with_no_built_in_price() {
        let overrides = BTreeMap::from([(
            "ollama/llama3".to_string(),
            ModelPrice {
                input: 0.5,
                output: 0.5,
                cache_read: 0.5,
                cache_write: 0.5,
                cache_write_long: 0.5,
            },
        )]);
        let usage = million(|usage, n| usage.uncached_input_tokens = Some(n));

        let table = PriceTable::new(&overrides);

        assert_usd(table.estimate("ollama", "llama3", &usage), 0.5);
    }

    #[test]
    fn a_dated_model_id_is_priced_as_its_base_model() {
        let usage = million(|usage, n| usage.uncached_input_tokens = Some(n));

        let table = PriceTable::new(&BTreeMap::new());

        assert_usd(
            table.estimate("anthropic", "claude-haiku-4-5-20251001", &usage),
            1.0,
        );
    }

    #[test]
    fn a_dated_model_id_prefers_an_override_for_that_exact_id() {
        let overrides = BTreeMap::from([(
            "anthropic/claude-haiku-4-5-20251001".to_string(),
            ModelPrice {
                input: 9.0,
                output: 5.0,
                cache_read: 0.1,
                cache_write: 1.25,
                cache_write_long: 2.0,
            },
        )]);
        let usage = million(|usage, n| usage.uncached_input_tokens = Some(n));

        let table = PriceTable::new(&overrides);

        assert_usd(
            table.estimate("anthropic", "claude-haiku-4-5-20251001", &usage),
            9.0,
        );
        assert_usd(table.estimate("anthropic", "claude-haiku-4-5", &usage), 1.0);
    }

    #[test]
    fn only_an_eight_digit_suffix_is_read_as_a_date() {
        let usage = million(|usage, n| usage.uncached_input_tokens = Some(n));

        let table = PriceTable::new(&BTreeMap::new());

        assert_eq!(
            table.estimate("anthropic", "claude-haiku-4-5-202510", &usage),
            None
        );
        assert_eq!(
            table.estimate("anthropic", "claude-haiku-4-5-latest", &usage),
            None
        );
    }

    #[test]
    fn the_gemini_and_fireworks_flash_entries_use_their_listed_rates() {
        let usage = Usage {
            uncached_input_tokens: Some(1_000_000),
            cache_read_tokens: Some(1_000_000),
            output_tokens: Some(1_000_000),
            ..Usage::default()
        };

        let table = PriceTable::new(&BTreeMap::new());

        // gemini-3.8-flash: $0.75 input, $0.075 cached, $3.75 output.
        assert_usd(table.estimate("gemini", "gemini-3.8-flash", &usage), 4.575);
        // glm-5p3-flash: $0.15 input, $0.03 cached, $0.50 output.
        assert_usd(
            table.estimate(
                "fireworks",
                "accounts/fireworks/models/glm-5p3-flash",
                &usage,
            ),
            0.68,
        );
    }

    #[test]
    fn every_built_in_key_names_a_known_provider_and_model() {
        for (key, _) in BUILT_IN {
            assert!(
                ModelSpec::from_str(key).is_ok(),
                "{key} is not a provider/model pair"
            );
        }
    }
}
