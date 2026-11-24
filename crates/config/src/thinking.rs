#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingConfig {
    Level(ThinkingLevel),
    Toggle(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLevel {
    Low,
    Medium,
    High,
}
