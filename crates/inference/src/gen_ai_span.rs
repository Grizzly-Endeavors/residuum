//! OpenTelemetry GenAI usage attributes on model-call spans.
//!
//! Each provider's request span declares the `gen_ai.usage.*` fields empty;
//! [`record_usage`] fills the ones the response reported, so an exported trace
//! carries what the call spent under the names trace tools read.

use crate::types::Usage;

/// Record `usage` on the current span. A count the provider didn't report
/// stays unset rather than becoming zero.
pub(crate) fn record_usage(usage: Option<&Usage>) {
    let Some(usage) = usage else {
        return;
    };
    let span = tracing::Span::current();
    let fields = [
        ("gen_ai.usage.input_tokens", usage.total_input_tokens()),
        ("gen_ai.usage.output_tokens", usage.output_tokens),
        (
            "gen_ai.usage.cache_read.input_tokens",
            usage.cache_read_tokens,
        ),
        (
            "gen_ai.usage.cache_write.input_tokens",
            usage.cache_write_tokens,
        ),
        (
            "gen_ai.usage.reasoning.output_tokens",
            usage.reasoning_tokens,
        ),
    ];
    for (name, value) in fields {
        if let Some(value) = value {
            span.record(name, value);
        }
    }
}

/// Record the model that answered on the current span.
pub(crate) fn record_response_model(model: &str) {
    if !model.is_empty() {
        tracing::Span::current().record("gen_ai.response.model", model);
    }
}
