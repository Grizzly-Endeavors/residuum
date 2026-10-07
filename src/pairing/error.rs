//! What can go wrong when pairing, in words the person pairing can act on.

/// A pairing operation that didn't happen, with the reason to show.
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    /// The pairing code, token or request is unknown, used, or expired.
    #[error("{0}")]
    Rejected(String),
    /// Too many attempts from one address, or too many at once overall.
    #[error("{0}")]
    RateLimited(String),
    /// The request isn't valid on its own terms (a name, a code's shape).
    #[error("{0}")]
    Invalid(String),
    /// The operation needs something that isn't set up yet.
    #[error("{0}")]
    NotReady(String),
    /// The pairing file couldn't be read or written.
    #[error("{0}")]
    Storage(String),
    /// The operating system's random source failed.
    #[error(
        "Residuum couldn't generate a secure random value. Try again, and check Residuum's logs if it keeps failing."
    )]
    Random,
}
