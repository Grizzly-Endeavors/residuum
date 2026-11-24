#[derive(Debug, thiserror::Error)]
pub enum FatalError {
    #[error("config error: {0}")]
    Config(String),

    #[error("workspace error: {0}")]
    Workspace(String),

    #[error("gateway error: {0}")]
    Gateway(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
