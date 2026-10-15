/// Thinking/reasoning configuration for model completions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingConfig {
    /// Graduated reasoning effort (Anthropic, `OpenAI`, Gemini).
    Level(ThinkingLevel),
    /// Simple on/off toggle (Ollama, or explicit disable).
    Toggle(bool),
}

/// Reasoning effort levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLevel {
    Low,
    Medium,
    High,
}
