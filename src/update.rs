//! Update checking and self-update logic.
//!
//! Provides version checking against GitHub Releases, binary replacement
//! via the install script, and shared update status for the gateway.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use anyhow::{Context, bail};

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

/// Fetch the latest release, update shared state, log on failure.
pub async fn check_for_update(status: &SharedUpdateStatus) {
    tracing::debug!("checking for updates");
    {
        let mut s = status.write().await;
        s.checking = true;
    }

    match fetch_latest_version().await {
        Ok(latest) => {
            let mut s = status.write().await;
            s.update_available = !is_up_to_date(&s.current, &latest);
            if s.update_available {
                tracing::info!(current = %s.current, latest = %latest, "update available");
            } else {
                tracing::debug!(current = %s.current, latest = %latest, "already up to date");
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
/// Returns an error if the HTTP request or JSON parsing fails.
pub async fn fetch_latest_version() -> anyhow::Result<String> {
    let url = "https://api.github.com/repos/grizzly-endeavors/residuum/releases/latest";
    tracing::debug!(url = %url, "fetching latest release");

    let client = reqwest::Client::builder()
        .user_agent("residuum-updater")
        .build()
        .context("failed to build http client")?;

    let resp = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("failed to fetch latest release")?;

    if !resp.status().is_success() {
        bail!("github api returned {} — are you online?", resp.status());
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse release response")?;

    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("release response missing tag_name field"))?;

    tracing::debug!(tag_name = %tag, "fetched latest release tag");
    Ok(tag)
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
    if current.starts_with(latest)
        && current
            .get(latest.len()..)
            .is_some_and(|r| r.starts_with('-'))
    {
        return true;
    }
    false
}

/// Download the latest release binary and replace the current executable.
///
/// Downloads directly from GitHub Releases, avoiding the install script
/// (which requires an interactive terminal for `sudo` on macOS).
///
/// # Errors
///
/// Returns an error if the download, platform detection,
/// or binary replacement fails.
pub async fn download_and_install(version: &str) -> anyhow::Result<()> {
    let platform = detect_platform()?;
    let url = format!(
        "https://github.com/grizzly-endeavors/residuum/releases/download/{version}/residuum-{platform}"
    );

    tracing::info!(version = %version, %platform, "downloading update binary");

    let client = reqwest::Client::builder()
        .user_agent("residuum-updater")
        .build()
        .context("failed to build http client")?;

    let response = client
        .get(&url)
        .send()
        .await
        .context("failed to download update binary")?;

    if !response.status().is_success() {
        bail!(
            "update binary download returned HTTP {} — asset may not exist for {platform}",
            response.status()
        );
    }

    let bytes = response
        .bytes()
        .await
        .context("failed to read update binary")?;

    tracing::debug!(bytes = bytes.len(), version = %version, "download complete");

    let current_exe =
        std::env::current_exe().context("failed to determine current executable path")?;

    // On Linux, the kernel appends " (deleted)" to /proc/self/exe when the
    // binary has been atomically replaced. Strip it to get the real path.
    let exe_path = current_exe
        .to_string_lossy()
        .strip_suffix(" (deleted)")
        .map(std::path::PathBuf::from)
        .unwrap_or(current_exe);

    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("current executable has no parent directory"))?;

    // Write to a temp file in the same directory for atomic rename
    let tmp_path = exe_dir.join(".residuum-update.tmp");

    let cleanup = || {
        if let Err(re) = std::fs::remove_file(&tmp_path) {
            tracing::warn!(error = %re, path = %tmp_path.display(), "failed to remove temp file during cleanup");
        }
    };

    std::fs::write(&tmp_path, &bytes).with_context(|| {
        format!(
            "failed to write update binary to {} — check directory permissions",
            tmp_path.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))
            .inspect_err(|_| cleanup())
            .context("failed to set executable permissions")?;
    }

    // Atomic rename replaces the binary on disk while the running process
    // keeps its handle to the old inode
    std::fs::rename(&tmp_path, &exe_path)
        .inspect_err(|_| cleanup())
        .with_context(|| {
            format!(
                "failed to replace binary at {} — check directory permissions",
                exe_path.display()
            )
        })?;

    tracing::info!(version = %version, path = %exe_path.display(), "update binary installed successfully");
    Ok(())
}

/// Detect the current platform in the format used by release asset names.
///
/// # Errors
///
/// Returns an error for unsupported OS/architecture combinations.
fn detect_platform() -> anyhow::Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => {
            bail!("unsupported operating system for self-update: {other}");
        }
    };

    let arch = match std::env::consts::ARCH {
        arch @ ("x86_64" | "aarch64") => arch,
        other => {
            bail!("unsupported architecture for self-update: {other}");
        }
    };

    if os == "darwin" && arch == "x86_64" {
        bail!("macOS x86_64 (Intel) is not supported — Apple Silicon only");
    }

    Ok(format!("{os}-{arch}"))
}

#[cfg(test)]
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
}
