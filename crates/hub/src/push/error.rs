#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("{0}")]
    BadRequest(String),
    #[error("there is no notification device with id '{0}'")]
    UnknownDevice(String),
    #[error("{0}")]
    Failed(String),
}
