//! Update checking and self-update logic.
//!
//! Provides version checking against GitHub Releases, binary replacement
//! via the install script, and shared update status for the gateway.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use crate::error::ResiduumError;

/// Build-time version injected by the release workflow.
pub const CURRENT_VERSION: &str = env!("RESIDUUM_VERSION");

/// Shared update status visible to the gateway event loop and web API.
#[derive(Debug, Clone)]
pub struct UpdateStatus {
    /// Current binary version.
    pub current: String,
    /// Latest release tag from GitHub (None if never checked).
    pub latest: Option<String>,
    /// Whether an update is available.
    pub update_available: bool,
    /// When the last successful check occurred.
    pub last_checked: Option<DateTime<Utc>>,
    /// Whether a check is currently in progress.
    pub checking: bool,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            current: CURRENT_VERSION.to_string(),
            latest: None,
            update_available: false,
            last_checked: None,
            checking: false,
        }
    }
}

/// Thread-safe shared update status.
pub type SharedUpdateStatus = Arc<RwLock<UpdateStatus>>;

/// Create a new shared update status with defaults.
#[must_use]
pub fn new_shared_status() -> SharedUpdateStatus {
    Arc::new(RwLock::new(UpdateStatus::default()))
}

/// Fetch the latest release, update shared state, log on failure.
pub async fn check_for_update(status: &SharedUpdateStatus) {
    {
        let mut s = status.write().await;
        s.checking = true;
    }

    match fetch_latest_version().await {
        Ok(latest) => {
            let mut s = status.write().await;
            let current = &s.current;
            s.update_available = !is_up_to_date(current, &latest);
            if s.update_available {
                tracing::info!(current = %s.current, latest = %latest, "update available");
            }
            s.latest = Some(latest);
            s.last_checked = Some(Utc::now());
            s.checking = false;
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to check for updates");
            let mut s = status.write().await;
            s.checking = false;
        }
    }
}

/// Fetch the latest release tag name from GitHub.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` if the HTTP request or JSON parsing fails.
pub async fn fetch_latest_version() -> Result<String, ResiduumError> {
    let client = reqwest::Client::builder()
        .user_agent("residuum-updater")
        .build()
        .map_err(|e| ResiduumError::Gateway(format!("failed to build http client: {e}")))?;

    let resp = client
        .get("https://api.github.com/repos/grizzly-endeavors/residuum/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to fetch latest release: {e}")))?;

    if !resp.status().is_success() {
        return Err(ResiduumError::Gateway(format!(
            "github api returned {} — are you online?",
            resp.status()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to parse release response: {e}")))?;

    body.get("tag_name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| {
            ResiduumError::Gateway("release response missing tag_name field".to_string())
        })
}

/// Compare the current build version against the latest release tag.
///
/// Returns `true` if the current version starts with the latest tag,
/// accounting for `git describe` suffixes like `-5-gabcdef1`.
#[must_use]
pub fn is_up_to_date(current: &str, latest: &str) -> bool {
    // Exact match (tagged commit)
    if current == latest {
        return true;
    }
    // current is "v2026.03.02-5-gabcdef1" and latest is "v2026.03.02" —
    // the current build is *ahead* of the latest release
    if current.starts_with(latest) && current.as_bytes().get(latest.len()) == Some(&b'-') {
        return true;
    }
    false
}

/// Download the latest release binary and replace the current executable.
///
/// Downloads directly from GitHub Releases, avoiding the install script
/// (which requires an interactive terminal for `sudo` on macOS).
///
/// Returns the version tag that was installed.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` if the download, platform detection,
/// or binary replacement fails.
pub async fn download_and_install() -> Result<String, ResiduumError> {
    let latest = fetch_latest_version().await?;
    let platform = detect_platform()?;
    let url = format!(
        "https://github.com/grizzly-endeavors/residuum/releases/download/{latest}/residuum-{platform}"
    );

    tracing::info!(version = %latest, %platform, "downloading update binary");

    let client = reqwest::Client::builder()
        .user_agent("residuum-updater")
        .build()
        .map_err(|e| ResiduumError::Gateway(format!("failed to build http client: {e}")))?;

    let response =
        client.get(&url).send().await.map_err(|e| {
            ResiduumError::Gateway(format!("failed to download update binary: {e}"))
        })?;

    if !response.status().is_success() {
        return Err(ResiduumError::Gateway(format!(
            "update binary download returned HTTP {} — asset may not exist for {platform}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to read update binary: {e}")))?;

    let current_exe = std::env::current_exe().map_err(|e| {
        ResiduumError::Gateway(format!("failed to determine current executable path: {e}"))
    })?;

    // On Linux, the kernel appends " (deleted)" to /proc/self/exe when the
    // binary has been atomically replaced. Strip it to get the real path.
    let exe_path = {
        let s = current_exe.to_string_lossy();
        if let Some(stripped) = s.strip_suffix(" (deleted)") {
            std::path::PathBuf::from(stripped)
        } else {
            current_exe
        }
    };

    let exe_dir = exe_path.parent().ok_or_else(|| {
        ResiduumError::Gateway("current executable has no parent directory".to_string())
    })?;

    // Write to a temp file in the same directory for atomic rename
    let tmp_path = exe_dir.join(".residuum-update.tmp");

    std::fs::write(&tmp_path, &bytes).map_err(|e| {
        ResiduumError::Gateway(format!(
            "failed to write update binary to {} — check directory permissions: {e}",
            tmp_path.display()
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755)).map_err(
            |e| {
                drop(std::fs::remove_file(&tmp_path));
                ResiduumError::Gateway(format!("failed to set executable permissions: {e}"))
            },
        )?;
    }

    // Atomic rename replaces the binary on disk while the running process
    // keeps its handle to the old inode
    std::fs::rename(&tmp_path, &exe_path).map_err(|e| {
        drop(std::fs::remove_file(&tmp_path));
        ResiduumError::Gateway(format!(
            "failed to replace binary at {} — check directory permissions: {e}",
            exe_path.display()
        ))
    })?;

    tracing::info!(version = %latest, path = %exe_path.display(), "update binary installed successfully");
    Ok(latest)
}

/// Detect the current platform in the format used by release asset names.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` for unsupported OS/architecture combinations.
fn detect_platform() -> Result<String, ResiduumError> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => {
            return Err(ResiduumError::Gateway(format!(
                "unsupported operating system for self-update: {other}"
            )));
        }
    };

    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => {
            return Err(ResiduumError::Gateway(format!(
                "unsupported architecture for self-update: {other}"
            )));
        }
    };

    if os == "darwin" && arch == "x86_64" {
        return Err(ResiduumError::Gateway(
            "macOS x86_64 (Intel) is not supported — Apple Silicon only".to_string(),
        ));
    }

    Ok(format!("{os}-{arch}"))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn is_up_to_date_exact_match() {
        assert!(
            is_up_to_date("v2026.03.02", "v2026.03.02"),
            "exact match should be up to date"
        );
    }

    #[test]
    fn is_up_to_date_ahead_of_release() {
        assert!(
            is_up_to_date("v2026.03.02-5-gabcdef1", "v2026.03.02"),
            "build ahead of latest release should be up to date"
        );
    }

    #[test]
    fn is_up_to_date_different_version() {
        assert!(
            !is_up_to_date("v2026.03.01", "v2026.03.02"),
            "older version should not be up to date"
        );
    }

    #[test]
    fn is_up_to_date_dev_build() {
        assert!(
            !is_up_to_date("dev", "v2026.03.02"),
            "dev build should not be up to date"
        );
    }

    #[test]
    fn is_up_to_date_no_false_prefix_match() {
        assert!(
            !is_up_to_date("v2026.03.021", "v2026.03.02"),
            "version with shared prefix but no dash separator should not match"
        );
    }

    #[test]
    fn default_status_uses_current_version() {
        let status = UpdateStatus::default();
        assert_eq!(status.current, CURRENT_VERSION);
        assert!(!status.update_available);
        assert!(status.latest.is_none());
        assert!(status.last_checked.is_none());
        assert!(!status.checking);
    }

    #[test]
    fn new_shared_status_is_default() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let shared = new_shared_status();
            let s = shared.read().await;
            assert_eq!(s.current, CURRENT_VERSION);
            assert!(!s.update_available);
        });
    }
}
