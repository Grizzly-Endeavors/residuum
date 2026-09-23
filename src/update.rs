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

/// Remove the `.exe.old` leftover from a previous Windows self-update.
///
/// Call this during startup. On non-Windows platforms this is a no-op.
pub fn cleanup_old_binary() {
    #[cfg(windows)]
    {
        if let Ok(exe) = std::env::current_exe() {
            let old = exe.with_extension("exe.old");
            if old.exists()
                && let Err(e) = std::fs::remove_file(&old)
            {
                tracing::debug!(path = %old.display(), error = %e, "could not remove old binary (may still be in use)");
            }
        }
    }
}

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
#[tracing::instrument(skip_all)]
pub async fn check_for_update(status: &SharedUpdateStatus) {
    tracing::trace!("checking for updates");
    {
        let mut s = status.write().await;
        s.checking = true;
    }

    let result = fetch_latest_version().await;
    let mut s = status.write().await;
    apply_fetch_result(&mut s, result);
}

fn apply_fetch_result(s: &mut UpdateStatus, result: anyhow::Result<String>) {
    match result {
        Ok(latest) => {
            s.update_available = !is_up_to_date(&s.current, &latest);
            if s.update_available {
                tracing::info!(current = %s.current, latest = %latest, "update available");
            } else {
                tracing::trace!(current = %s.current, latest = %latest, "already up to date");
            }
            s.latest = Some(latest);
            s.last_checked = Some(Utc::now());
            s.checking = false;
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to check for updates");
            s.checking = false;
        }
    }
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("residuum-updater")
        .build()
        .context("failed to build http client")
}

/// Fetch the latest release tag name from GitHub.
///
/// # Errors
///
/// Returns an error if the HTTP request or JSON parsing fails.
#[tracing::instrument(skip_all)]
pub async fn fetch_latest_version() -> anyhow::Result<String> {
    let client = http_client()?;

    let resp = client
        .get("https://api.github.com/repos/grizzly-endeavors/residuum/releases/latest")
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
/// accounting for `git describe` suffixes like `-5-gabcdef1`, or if the
/// current version is a newer `CalVer` release than the latest.
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
    // current is a newer tagged release (e.g. rollback or delayed publish)
    if let (Some(cur_ver), Some(lat_ver)) = (parse_calver(current), parse_calver(latest))
        && cur_ver > lat_ver
    {
        return true;
    }
    false
}

fn parse_calver(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.strip_prefix('v')?;
    let mut parts = v.splitn(3, '.');
    let year: u32 = parts.next()?.parse().ok()?;
    let month_str = parts.next()?;
    let day_str = parts.next()?;
    if month_str.len() != 2 || day_str.len() != 2 {
        return None;
    }
    let month: u32 = month_str.parse().ok()?;
    let day: u32 = day_str.parse().ok()?;
    Some((year, month, day))
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
#[tracing::instrument(skip_all, fields(version = %version))]
pub async fn download_and_install(version: &str) -> anyhow::Result<()> {
    let asset = release_asset_name(std::env::consts::OS, std::env::consts::ARCH)?;
    let url = format!(
        "https://github.com/grizzly-endeavors/residuum/releases/download/{version}/{asset}"
    );

    tracing::info!(version = %version, %asset, "downloading update binary");

    let client = http_client()?;

    let response = client
        .get(&url)
        .send()
        .await
        .context("failed to download update binary")?;

    if !response.status().is_success() {
        bail!(
            "update binary download returned HTTP {} — release asset {asset} may not exist for {version}",
            response.status()
        );
    }

    let bytes = response
        .bytes()
        .await
        .context("failed to read update binary")?;

    tracing::debug!(bytes = bytes.len(), "download complete");

    let current_exe =
        std::env::current_exe().context("failed to determine current executable path")?;

    // On Linux, the kernel appends " (deleted)" to /proc/self/exe when the
    // binary has been atomically replaced. Strip it to get the real path.
    #[cfg(target_os = "linux")]
    let exe_path = current_exe
        .to_string_lossy()
        .strip_suffix(" (deleted)")
        .map(std::path::PathBuf::from)
        .unwrap_or(current_exe);
    #[cfg(not(target_os = "linux"))]
    let exe_path = current_exe;

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

    std::fs::write(&tmp_path, &bytes)
        .inspect_err(|_| cleanup())
        .with_context(|| {
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

    // On Windows, the running exe can't be overwritten directly. Rename it
    // out of the way first, then move the new binary into place. The old
    // binary is cleaned up on next startup via `cleanup_old_binary`.
    #[cfg(windows)]
    {
        let old_path = exe_path.with_extension("exe.old");
        // Remove any leftover .old from a previous update
        if let Err(e) = std::fs::remove_file(&old_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %old_path.display(), error = %e, "failed to remove leftover .exe.old before update");
        }
        std::fs::rename(&exe_path, &old_path)
            .inspect_err(|_| cleanup())
            .with_context(|| {
                format!(
                    "failed to rename running binary at {} — an antivirus may be holding the file",
                    exe_path.display()
                )
            })?;
    }

    // Atomic rename replaces the binary on disk (Unix: running process keeps
    // its handle to the old inode; Windows: old binary already moved above)
    std::fs::rename(&tmp_path, &exe_path)
        .inspect_err(|_| cleanup())
        .with_context(|| {
            format!(
                "failed to replace binary at {} — check directory permissions",
                exe_path.display()
            )
        })?;

    tracing::info!(path = %exe_path.display(), "update binary installed successfully");
    Ok(())
}

/// Name of the release asset built for `os`/`arch` (values of
/// `std::env::consts::OS`/`ARCH`), exactly as `release.yml` uploads it.
///
/// # Errors
///
/// Returns an error for OS/architecture combinations that have no release build.
fn release_asset_name(os: &str, arch: &str) -> anyhow::Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("residuum-linux-x86_64"),
        ("linux", "aarch64") => Ok("residuum-linux-aarch64"),
        ("macos", "aarch64") => Ok("residuum-macos-aarch64"),
        ("windows", "x86_64") => Ok("residuum-windows-x86_64.exe"),
        ("macos", "x86_64") => bail!("macOS x86_64 (Intel) is not supported — Apple Silicon only"),
        (os, arch) => {
            bail!("no release build exists for {os} on {arch}, so self-update isn't available")
        }
    }
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
    fn is_up_to_date_current_newer_than_latest() {
        assert!(
            is_up_to_date("v2026.03.03", "v2026.03.02"),
            "version newer than latest release should be up to date"
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
    fn apply_fetch_result_clears_checking_on_error() {
        let mut s = UpdateStatus {
            checking: true,
            ..Default::default()
        };
        apply_fetch_result(&mut s, Err(anyhow::anyhow!("network error")));
        assert!(!s.checking, "checking flag should be cleared on error");
    }

    #[test]
    fn apply_fetch_result_clears_checking_on_success() {
        let mut s = UpdateStatus {
            checking: true,
            ..Default::default()
        };
        apply_fetch_result(&mut s, Ok("v2026.03.02".to_string()));
        assert!(!s.checking, "checking flag should be cleared on success");
    }

    #[test]
    fn release_asset_name_matches_every_release_build() {
        // The names must match what the release workflow uploads, or
        // self-update downloads a URL that 404s.
        let release_workflow = include_str!("../.github/workflows/release.yml");
        for (os, arch) in [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("macos", "aarch64"),
            ("windows", "x86_64"),
        ] {
            let asset = release_asset_name(os, arch).unwrap();
            let expected = format!("artifact: {asset}");
            assert!(
                release_workflow.lines().any(|line| line.trim() == expected),
                "{os}/{arch} maps to {asset}, which release.yml does not build"
            );
        }
    }

    #[test]
    fn release_asset_name_rejects_platforms_without_a_build() {
        assert!(release_asset_name("macos", "x86_64").is_err());
        assert!(release_asset_name("freebsd", "x86_64").is_err());
        assert!(release_asset_name("linux", "riscv64").is_err());
    }

    #[test]
    fn release_asset_name_supports_the_current_platform() {
        let result = release_asset_name(std::env::consts::OS, std::env::consts::ARCH);
        assert!(
            result.is_ok(),
            "no release asset for this platform: {result:?}"
        );
    }
}
