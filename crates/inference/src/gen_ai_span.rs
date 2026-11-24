use residuum_config::ProviderKind;

use crate::types::Usage;

pub(crate) const CHAT_OPERATION: &str = "chat";

pub(crate) fn provider_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Fireworks => "fireworks",
        ProviderKind::Gemini => "gcp.gemini",
        ProviderKind::Ollama => "ollama",
        ProviderKind::OpenAi => "openai",
    }
}

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

pub(crate) fn record_response_model(model: &str) {
    if !model.is_empty() {
        tracing::Span::current().record("gen_ai.response.model", model);
    }
}
