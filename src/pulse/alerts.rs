use std::path::Path;

use crate::error::IronclawError;

/// Load the content of Alerts.md.
///
/// Returns `None` if the file does not exist; `Some(content)` if it does.
///
/// # Errors
///
/// Returns `IronclawError::Workspace` if the file exists but cannot be read.
pub async fn load_alerts(path: &Path) -> Result<Option<String>, IronclawError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(IronclawError::Workspace(format!(
            "failed to read Alerts.md at {}: {e}",
            path.display()
        ))),
    }
}
