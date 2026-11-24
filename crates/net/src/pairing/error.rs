#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    RateLimited(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotReady(String),
    #[error("{0}")]
    Storage(String),
    #[error(
        "Residuum couldn't generate a secure random value. Try again, and check Residuum's logs if it keeps failing."
    )]
    Random,
}
