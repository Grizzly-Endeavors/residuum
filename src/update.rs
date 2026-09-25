//! Update checking and self-update logic.
//!
//! Provides version checking against GitHub Releases, binary replacement
//! via the install script, and shared update status for the gateway.
//!
//! A self-update never leaves only a possibly-broken binary on disk: the
//! previously-running binary is preserved beside the new one
//! ([`previous_binary_path`]) and a [`PendingRollback`] marker records where
//! it is. `commands::serve::foreground::relaunch` reads that marker to
//! decide whether the restart needs the rollback-capable watchdog path or
//! the plain one, and the watchdog deletes the preserved binary once the new
//! version proves healthy (see `commands::update_watchdog`) or restores it
//! and leaves a [`RollbackNotice`] otherwise.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

/// The path a running binary is preserved at for rollback while a new
/// version installed at `exe` is proven healthy.
///
/// Appends rather than replaces the extension (`residuum` →
/// `residuum.prev`, `residuum.exe` → `residuum.exe.prev`) so the preserved
/// file keeps whatever extension the platform needs to execute it directly.
#[must_use]
pub fn previous_binary_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(".prev");
    PathBuf::from(name)
}

/// Download the latest release binary and install it, preserving the
/// binary that was running so a failed restart can roll back to it.
///
/// Downloads directly from GitHub Releases, avoiding the install script
/// (which requires an interactive terminal for `sudo` on macOS). GitHub
/// Releases publish no checksum or signature for these artifacts (see
/// `.github/workflows/release.yml`), so there is nothing to verify the
/// download against beyond the HTTPS connection itself.
///
/// Returns the path the previous binary was preserved at, and leaves a
/// [`PendingRollback`] marker there for `commands::serve::foreground::relaunch`
/// to act on once the restart it triggers actually happens.
///
/// # Errors
///
/// Returns an error if the download, platform detection, or binary
/// replacement fails.
#[tracing::instrument(skip_all, fields(version = %version))]
pub async fn download_and_install(version: &str) -> anyhow::Result<PathBuf> {
    let bytes = download_release_bytes(version).await?;
    let exe_path = running_exe_path().context("failed to determine current executable path")?;
    let prev_path = swap_in_new_binary(&exe_path, &bytes)?;

    tracing::info!(path = %exe_path.display(), previous = %prev_path.display(), "update binary installed, previous version preserved for rollback");
    record_pending_rollback(&prev_path, version);
    Ok(prev_path)
}

/// Download the release asset for the current platform's bytes.
async fn download_release_bytes(version: &str) -> anyhow::Result<Vec<u8>> {
    let asset = release_asset_name(std::env::consts::OS, std::env::consts::ARCH)?;
    let url = format!(
        "https://github.com/grizzly-endeavors/residuum/releases/download/{version}/{asset}"
    );

    tracing::info!(version = %version, %asset, "downloading update binary");

    let response = http_client()?
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
    Ok(bytes.to_vec())
}

/// The currently-running binary's live path on disk.
///
/// On Linux, the kernel appends " (deleted)" to `/proc/self/exe` once the
/// binary has been atomically replaced — this strips that suffix so the
/// path is the real one to rename, not the stale in-memory one.
fn running_exe_path() -> std::io::Result<PathBuf> {
    let current_exe = std::env::current_exe()?;
    #[cfg(target_os = "linux")]
    {
        Ok(current_exe
            .to_string_lossy()
            .strip_suffix(" (deleted)")
            .map(PathBuf::from)
            .unwrap_or(current_exe))
    }
    #[cfg(not(target_os = "linux"))]
    Ok(current_exe)
}

/// Write `bytes` to a temp file beside `exe_path`, preserve the currently
/// running binary at [`previous_binary_path`], then swap the new one into
/// `exe_path`. Restores the original binary and returns an error if the
/// final swap fails, so a partial update never leaves nothing executable.
///
/// # Errors
///
/// Returns an error if `exe_path` has no parent directory, or if writing,
/// permissioning, or renaming any of the files fails.
fn swap_in_new_binary(exe_path: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("current executable has no parent directory"))?;
    let tmp_path = exe_dir.join(".residuum-update.tmp");
    let cleanup = || {
        if let Err(re) = std::fs::remove_file(&tmp_path) {
            tracing::warn!(error = %re, path = %tmp_path.display(), "failed to remove temp file during cleanup");
        }
    };

    std::fs::write(&tmp_path, bytes)
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

    let prev_path = previous_binary_path(exe_path);

    // Clear a stale backup from an update that was applied but never
    // confirmed healthy (or never cleaned up after a crash) — only the
    // binary this update is about to replace should be preserved.
    if let Err(e) = std::fs::remove_file(&prev_path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %prev_path.display(), error = %e, "failed to remove stale previous-binary backup before update");
    }

    // The running binary can't be overwritten in place — on Windows this is
    // enforced by the OS, and on Unix it would otherwise leave nothing to
    // roll back to. Rename it out of the way first, then move the new
    // binary into place.
    std::fs::rename(exe_path, &prev_path)
        .inspect_err(|_| cleanup())
        .with_context(|| {
            format!(
                "failed to preserve the running binary at {} for rollback",
                prev_path.display()
            )
        })?;

    if let Err(e) = std::fs::rename(&tmp_path, exe_path) {
        // Restore the running binary's original name so this failure
        // doesn't leave nothing executable at exe_path.
        if let Err(re) = std::fs::rename(&prev_path, exe_path) {
            tracing::error!(error = %re, path = %exe_path.display(), "failed to restore the running binary after a failed update swap");
        }
        cleanup();
        return Err(e).with_context(|| {
            format!(
                "failed to install the new binary at {} — check directory permissions",
                exe_path.display()
            )
        });
    }

    Ok(prev_path)
}

/// Leave a [`PendingRollback`] marker for the restart that will follow, and
/// clear any stale rollback notice from an earlier attempt. Best-effort: if
/// the config directory can't be found, the update still installed
/// correctly, it just won't be able to roll back on a failed restart.
fn record_pending_rollback(prev_path: &Path, target_version: &str) {
    let Ok(config_dir) = crate::config::Config::config_dir() else {
        tracing::warn!(
            "could not determine config directory; the next restart won't be able to roll back this update on failure"
        );
        return;
    };
    write_pending_rollback(
        &config_dir,
        &PendingRollback {
            previous_binary: prev_path.to_path_buf(),
            previous_version: CURRENT_VERSION.to_string(),
            target_version: target_version.to_string(),
        },
    );
    clear_rollback_notice(&config_dir);
}

/// What a restart needs to know to roll back an update if the new version
/// doesn't become healthy: where the previous binary was preserved, and
/// both versions involved (for the notice a rollback leaves behind).
///
/// Written by [`download_and_install`], read and deleted by
/// `commands::serve::foreground::relaunch` when it hands off to the
/// rollback-capable watchdog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingRollback {
    pub previous_binary: PathBuf,
    pub previous_version: String,
    pub target_version: String,
}

/// Path to the [`PendingRollback`] marker.
#[must_use]
pub fn pending_rollback_path(config_dir: &Path) -> PathBuf {
    config_dir.join("residuum.update-pending.json")
}

/// Record that a restart, once it happens, needs the rollback-capable path.
fn write_pending_rollback(config_dir: &Path, pending: &PendingRollback) {
    let path = pending_rollback_path(config_dir);
    match serde_json::to_string(pending) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(path = %path.display(), error = %e, "failed to write pending-rollback marker");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize pending-rollback marker");
        }
    }
}

/// Read and consume the [`PendingRollback`] marker, if one is present.
///
/// A malformed marker is treated the same as a missing one (logged, not
/// fatal): the restart just takes the plain, non-rollback-capable path
/// instead of failing outright.
#[must_use]
pub fn take_pending_rollback(config_dir: &Path) -> Option<PendingRollback> {
    let path = pending_rollback_path(config_dir);
    let raw = std::fs::read_to_string(&path).ok()?;
    if let Err(e) = std::fs::remove_file(&path) {
        tracing::warn!(path = %path.display(), error = %e, "failed to remove pending-rollback marker");
    }
    match serde_json::from_str(&raw) {
        Ok(pending) => Some(pending),
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse pending-rollback marker, ignoring it");
            None
        }
    }
}

/// Why an update-rollback watchdog restored the previous version, for the
/// user to see. Written by `commands::update_watchdog`, read by the update
/// status API and cleared at the start of the next update attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackNotice {
    pub attempted_version: String,
    pub reason: String,
    pub at: DateTime<Utc>,
}

/// Path to the [`RollbackNotice`] marker.
#[must_use]
pub fn rollback_notice_path(config_dir: &Path) -> PathBuf {
    config_dir.join("residuum.rollback-notice.json")
}

/// Record why a rollback happened, for the update status API to surface.
pub fn write_rollback_notice(config_dir: &Path, notice: &RollbackNotice) {
    let path = rollback_notice_path(config_dir);
    match serde_json::to_string(notice) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(path = %path.display(), error = %e, "failed to write rollback notice");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize rollback notice");
        }
    }
}

/// Read the current [`RollbackNotice`], if any.
#[must_use]
pub fn read_rollback_notice(config_dir: &Path) -> Option<RollbackNotice> {
    let raw = std::fs::read_to_string(rollback_notice_path(config_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Clear a rollback notice, e.g. before a new update attempt.
pub fn clear_rollback_notice(config_dir: &Path) {
    let path = rollback_notice_path(config_dir);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "failed to remove rollback notice");
    }
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

    #[test]
    fn previous_binary_path_appends_suffix_without_replacing_extension() {
        assert_eq!(
            previous_binary_path(Path::new("/opt/residuum/residuum")),
            PathBuf::from("/opt/residuum/residuum.prev")
        );
        assert_eq!(
            previous_binary_path(Path::new(r"C:\residuum\residuum.exe")),
            PathBuf::from(r"C:\residuum\residuum.exe.prev")
        );
    }

    #[test]
    fn pending_rollback_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(take_pending_rollback(dir.path()).is_none());

        let pending = PendingRollback {
            previous_binary: dir.path().join("residuum.prev"),
            previous_version: "v2026.09.01".to_string(),
            target_version: "v2026.09.24".to_string(),
        };
        write_pending_rollback(dir.path(), &pending);
        assert_eq!(take_pending_rollback(dir.path()), Some(pending));
        // Consumed: a second read finds nothing.
        assert!(take_pending_rollback(dir.path()).is_none());
    }

    #[test]
    fn pending_rollback_missing_marker_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(take_pending_rollback(dir.path()).is_none());
    }

    #[test]
    fn pending_rollback_malformed_marker_is_ignored_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(pending_rollback_path(dir.path()), "not json").unwrap();
        assert!(take_pending_rollback(dir.path()).is_none());
    }

    #[test]
    fn rollback_notice_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_rollback_notice(dir.path()).is_none());

        let notice = RollbackNotice {
            attempted_version: "v2026.09.24".to_string(),
            reason: "did not become healthy within 60 seconds".to_string(),
            at: Utc::now(),
        };
        write_rollback_notice(dir.path(), &notice);
        assert_eq!(read_rollback_notice(dir.path()), Some(notice));

        clear_rollback_notice(dir.path());
        assert!(read_rollback_notice(dir.path()).is_none());
    }
}
