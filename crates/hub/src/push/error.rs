//! The failures of the Web Push API, and the statuses the HTTP routes give
//! them.

/// Why a Web Push operation failed. Each message is plain words the user can
/// read.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// The request can't be used as given.
    #[error("{0}")]
    BadRequest(String),
    /// No device has this id.
    #[error("there is no notification device with id '{0}'")]
    UnknownDevice(String),
    /// The key or device file can't be read or written.
    #[error("{0}")]
    Failed(String),
}
