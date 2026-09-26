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
//!
//! Before that swap, the download is checked against the `SHA256SUMS`
//! asset published with the release. A mismatch refuses the install and
//! leaves the running binary where it is. A release that has no manifest
//! still installs, and leaves an [`UnverifiedUpdate`] the Update page shows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use anyhow::{Context, bail};

/// Build-time version injected by the release workflow.
pub const CURRENT_VERSION: &str = env!("RESIDUUM_VERSION");

/// Release asset that lists a SHA-256 hash for every other asset.
///
/// Written by `.github/workflows/release.yml` and downloaded next to the
/// binary. The name has to match the uploaded asset exactly.
const CHECKSUM_MANIFEST_ASSET: &str = "SHA256SUMS";

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

/// Where the previous binary was preserved, and whether the download
/// matched the release checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledUpdate {
    pub previous_binary: PathBuf,
    /// [`ReleaseVerification::Unverified`] when the release published no
    /// checksum manifest. The binary was still installed.
    pub verification: ReleaseVerification,
}

/// Whether the installed bytes matched `SHA256SUMS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseVerification {
    /// The download's SHA-256 matched the manifest entry for this asset.
    Verified,
    /// The release has no `SHA256SUMS` asset. The binary was installed,
    /// and callers should say that it couldn't be verified.
    Unverified,
}

/// An update was refused before the running binary was replaced.
///
/// The message is the whole user-facing explanation.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct RejectedUpdate {
    message: &'static str,
}

fn rejected(message: &'static str) -> anyhow::Error {
    anyhow::Error::from(RejectedUpdate { message })
}

/// The plain-language reason an update was refused before install, if
/// `err` is that refusal.
///
/// Other failures (a download that never arrived, a swap that couldn't
/// rename the binary) return `None`.
#[must_use]
pub fn rejection_message(err: &anyhow::Error) -> Option<&'static str> {
    err.downcast_ref::<RejectedUpdate>()
        .map(|rejected| rejected.message)
}

/// Download the latest release binary and install it, preserving the
/// binary that was running so a failed restart can roll back to it.
///
/// Downloads directly from GitHub Releases, avoiding the install script
/// (which requires an interactive terminal for `sudo` on macOS). The
/// download is checked against the release `SHA256SUMS` asset before the
/// running binary is moved. A mismatch returns an error and leaves the
/// current binary untouched. A release with no manifest still installs.
///
/// Leaves a [`PendingRollback`] marker for
/// `commands::serve::foreground::relaunch` to act on once the restart it
/// triggers actually happens. When the release had no checksum, also
/// leaves an [`UnverifiedUpdate`] marker.
///
/// # Errors
///
/// Returns an error if the download, checksum check, platform detection,
/// or binary replacement fails. A checksum mismatch is a
/// [`rejection_message`] and does not replace the running binary.
#[tracing::instrument(skip_all, fields(version = %version))]
pub async fn download_and_install(version: &str) -> anyhow::Result<InstalledUpdate> {
    let asset = release_asset_name(std::env::consts::OS, std::env::consts::ARCH)?;
    let bytes = download_release_bytes(version, asset).await?;
    let verification = verify_downloaded_asset(version, asset, &bytes).await;
    let exe_path = running_exe_path().context("failed to determine current executable path")?;
    let installed = install_verified_bytes(&exe_path, &bytes, verification)?;

    tracing::info!(
        path = %exe_path.display(),
        previous = %installed.previous_binary.display(),
        verified = matches!(installed.verification, ReleaseVerification::Verified),
        "update binary installed, previous version preserved for rollback"
    );
    record_pending_rollback(&installed.previous_binary, version);
    record_verification(version, installed.verification);
    Ok(installed)
}

/// Replace `exe_path` only when `verification` succeeded.
///
/// The `Result` is the gate: a mismatch (or any other refusal) returns
/// before [`swap_in_new_binary`], so a bad download cannot move the
/// running binary aside.
fn install_verified_bytes(
    exe_path: &Path,
    bytes: &[u8],
    verification: anyhow::Result<ReleaseVerification>,
) -> anyhow::Result<InstalledUpdate> {
    let verification = verification?;
    let previous_binary = swap_in_new_binary(exe_path, bytes)?;
    Ok(InstalledUpdate {
        previous_binary,
        verification,
    })
}

/// Download the release asset for this platform.
async fn download_release_bytes(version: &str, asset: &str) -> anyhow::Result<Vec<u8>> {
    let url = release_asset_url(version, asset);

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

fn release_asset_url(version: &str, asset: &str) -> String {
    format!("https://github.com/grizzly-endeavors/residuum/releases/download/{version}/{asset}")
}

/// Download `SHA256SUMS` and check `bytes` against the entry for `asset`.
///
/// A 404 is "this release didn't publish a manifest" and installs
/// unverified. Any other failure to obtain or read a usable manifest
/// refuses the install: the release may have a checksum we simply didn't
/// get, and installing past that would hide it.
async fn verify_downloaded_asset(
    version: &str,
    asset: &str,
    bytes: &[u8],
) -> anyhow::Result<ReleaseVerification> {
    let url = release_asset_url(version, CHECKSUM_MANIFEST_ASSET);
    tracing::info!(version = %version, asset = CHECKSUM_MANIFEST_ASSET, "downloading update checksum");

    let response = match http_client()?.get(&url).send().await {
        Ok(response) => response,
        Err(e) => {
            tracing::error!(error = %e, "failed to download update checksum");
            return Err(rejected(
                "couldn't verify this download — check your connection and try again",
            ));
        }
    };
    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => {
            tracing::error!(error = %e, "failed to read update checksum");
            return Err(rejected(
                "couldn't verify this download — check your connection and try again",
            ));
        }
    };
    let manifest = manifest_from_response(status, body)?;
    if manifest.is_none() {
        tracing::warn!(
            version,
            asset,
            "release has no checksum manifest; the update will be installed without one"
        );
    }
    verify_download(manifest.as_deref(), asset, bytes)
}

/// Map a checksum-asset response to a manifest body.
///
/// `Ok(None)` is a missing asset (HTTP 404). Other non-success statuses
/// refuse the install.
fn manifest_from_response(
    status: reqwest::StatusCode,
    body: String,
) -> anyhow::Result<Option<String>> {
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        tracing::error!(%status, "update checksum download failed");
        return Err(rejected(
            "couldn't verify this download — check your connection and try again",
        ));
    }
    Ok(Some(body))
}

/// Check `bytes` against a manifest body.
///
/// `manifest == None` means the release published no checksum file.
fn verify_download(
    manifest: Option<&str>,
    asset: &str,
    bytes: &[u8],
) -> anyhow::Result<ReleaseVerification> {
    let Some(body) = manifest else {
        return Ok(ReleaseVerification::Unverified);
    };
    match manifest_hash(body, asset) {
        ManifestLookup::Hash(expected) => {
            let actual = sha256_hex(bytes);
            if !expected.eq_ignore_ascii_case(&actual) {
                tracing::error!(
                    asset,
                    expected = %expected,
                    actual = %actual,
                    "update binary checksum did not match the release manifest"
                );
                return Err(rejected(
                    "the download was corrupt or incomplete — try again",
                ));
            }
            tracing::debug!(asset, "update binary matched the release checksum");
            Ok(ReleaseVerification::Verified)
        }
        ManifestLookup::AssetMissing => {
            tracing::error!(asset, "checksum manifest does not list this release binary");
            Err(rejected(
                "couldn't verify this download — the release checksum doesn't include this file, try again",
            ))
        }
        ManifestLookup::Unreadable => {
            tracing::error!("checksum manifest could not be read");
            Err(rejected(
                "couldn't verify this download — the release checksum was unreadable, try again",
            ))
        }
    }
}

enum ManifestLookup {
    Hash(String),
    /// The manifest has entries, and none of them is `asset`.
    AssetMissing,
    /// No usable `hash  filename` lines.
    Unreadable,
}

/// GNU `sha256sum` text (`hash  name`) and binary (`hash *name`) lines.
///
/// Blank lines, `#` comments, and lines that aren't a 64-digit hash plus
/// a filename are skipped. The first entry for a name wins.
struct ChecksumManifest {
    entries: Vec<(String, String)>,
}

impl ChecksumManifest {
    fn parse(body: &str) -> Self {
        let mut entries = Vec::new();
        for line in body.lines() {
            let Some((hash, name)) = parse_checksum_line(line) else {
                continue;
            };
            if entries.iter().any(|(existing, _)| existing == name) {
                continue;
            }
            entries.push((name.to_string(), hash.to_ascii_lowercase()));
        }
        Self { entries }
    }

    fn hash_for(&self, asset: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(name, _)| name == asset)
            .map(|(_, hash)| hash.as_str())
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn manifest_hash(body: &str, asset: &str) -> ManifestLookup {
    let manifest = ChecksumManifest::parse(body);
    if let Some(hash) = manifest.hash_for(asset) {
        return ManifestLookup::Hash(hash.to_string());
    }
    if manifest.is_empty() {
        ManifestLookup::Unreadable
    } else {
        ManifestLookup::AssetMissing
    }
}

fn parse_checksum_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (hash, rest) = line.split_once(char::is_whitespace)?;
    let name = rest.trim();
    let name = name.strip_prefix('*').unwrap_or(name);
    if !is_sha256_hex(hash) || name.is_empty() || name.chars().any(char::is_whitespace) {
        return None;
    }
    Some((hash, name))
}

fn is_sha256_hex(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut out = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        let hi = usize::from(byte >> 4);
        let lo = usize::from(byte & 0x0f);
        // A nibble is 0..=15, so both lookups hit.
        if let (Some(high), Some(low)) = (HEX.get(hi), HEX.get(lo)) {
            out.push(*high);
            out.push(*low);
        }
    }
    out
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

/// Record whether this install matched a checksum, for the Update page.
///
/// A verified install clears any earlier note. An unverified one replaces
/// it. Best-effort, same as [`record_pending_rollback`]: the binary is
/// already in place either way.
fn record_verification(version: &str, verification: ReleaseVerification) {
    let Ok(config_dir) = crate::config::Config::config_dir() else {
        tracing::warn!(
            "could not determine config directory; the update page won't show whether this update was verified"
        );
        return;
    };
    match verification {
        ReleaseVerification::Verified => clear_unverified_update(&config_dir),
        ReleaseVerification::Unverified => write_unverified_update(
            &config_dir,
            &UnverifiedUpdate {
                version: version.to_string(),
                at: Utc::now(),
            },
        ),
    }
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
///
/// Clears an [`UnverifiedUpdate`] too: that note describes the version
/// that was just installed, and a rollback means it isn't running anymore.
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
    clear_unverified_update(config_dir);
}

/// An update that installed even though the release published no checksum.
///
/// Written by [`download_and_install`] when verification is
/// [`ReleaseVerification::Unverified`], read by the update status API, and
/// cleared by the next verified install or by [`write_rollback_notice`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnverifiedUpdate {
    pub version: String,
    pub at: DateTime<Utc>,
}

/// Path to the [`UnverifiedUpdate`] marker.
#[must_use]
pub fn unverified_update_path(config_dir: &Path) -> PathBuf {
    config_dir.join("residuum.update-unverified.json")
}

/// Record that the installed update had no checksum to check it against.
pub fn write_unverified_update(config_dir: &Path, notice: &UnverifiedUpdate) {
    let path = unverified_update_path(config_dir);
    match serde_json::to_string(notice) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(path = %path.display(), error = %e, "failed to write unverified-update notice");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize unverified-update notice");
        }
    }
}

/// Read the current [`UnverifiedUpdate`], if any.
#[must_use]
pub fn read_unverified_update(config_dir: &Path) -> Option<UnverifiedUpdate> {
    let raw = std::fs::read_to_string(unverified_update_path(config_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Clear an unverified-update notice, e.g. after a verified install.
pub fn clear_unverified_update(config_dir: &Path) {
    let path = unverified_update_path(config_dir);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "failed to remove unverified-update notice");
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
        let manifest_upload = format!("ASSETS+=(\"{CHECKSUM_MANIFEST_ASSET}\")");
        assert!(
            release_workflow
                .lines()
                .any(|line| line.trim() == manifest_upload),
            "release.yml must upload {CHECKSUM_MANIFEST_ASSET} alongside the binaries"
        );
        assert!(
            release_workflow.contains("sha256sum"),
            "release.yml must hash release binaries into {CHECKSUM_MANIFEST_ASSET}"
        );
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

    #[test]
    fn sha256_hex_matches_a_known_digest() {
        // SHA-256("abc") from FIPS 180-2.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parses_sha256sums_manifest() {
        let linux = sha256_hex(b"linux-bytes");
        let windows = sha256_hex(b"windows-bytes");
        let body = format!(
            "\
# comment

{linux}  residuum-linux-x86_64
{windows_upper} *residuum-windows-x86_64.exe
not-a-hash  ignored
",
            windows_upper = windows.to_ascii_uppercase()
        );

        let manifest = ChecksumManifest::parse(&body);
        assert_eq!(
            manifest.hash_for("residuum-linux-x86_64"),
            Some(linux.as_str())
        );
        assert_eq!(
            manifest.hash_for("residuum-windows-x86_64.exe"),
            Some(windows.as_str()),
            "binary-mode lines and uppercase hex should parse"
        );
        assert!(
            manifest.hash_for("ignored").is_none(),
            "a line that isn't a sha256 hash should be skipped"
        );
        assert!(
            manifest.hash_for("residuum-linux-aarch64").is_none(),
            "an asset the manifest doesn't list should be absent"
        );
    }

    #[test]
    fn checksum_mismatch_refuses_install_and_leaves_the_binary_untouched() {
        let manifest = format!("{}  residuum-linux-x86_64\n", sha256_hex(b"good-bytes"));
        let decision = verify_download(Some(&manifest), "residuum-linux-x86_64", b"corrupt");

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("residuum");
        std::fs::write(&exe, b"current").unwrap();

        let err = install_verified_bytes(&exe, b"corrupt", decision).unwrap_err();
        assert_eq!(
            rejection_message(&err),
            Some("the download was corrupt or incomplete — try again")
        );
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"current",
            "a mismatched download must not replace the running binary"
        );
        assert!(
            !previous_binary_path(&exe).exists(),
            "a mismatched download must not move the running binary aside"
        );
        assert!(
            !dir.path().join(".residuum-update.tmp").exists(),
            "a mismatched download must not leave a temp binary behind"
        );
    }

    #[test]
    fn missing_manifest_installs_and_is_unverified() {
        let decision = verify_download(None, "residuum-linux-x86_64", b"new-bytes");
        assert_eq!(
            decision.as_ref().unwrap(),
            &ReleaseVerification::Unverified,
            "a release with no checksum manifest should still be installable"
        );

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("residuum");
        std::fs::write(&exe, b"current").unwrap();

        let installed = install_verified_bytes(&exe, b"new-bytes", decision).unwrap();
        assert_eq!(installed.verification, ReleaseVerification::Unverified);
        assert_eq!(std::fs::read(&exe).unwrap(), b"new-bytes");
        assert_eq!(
            std::fs::read(previous_binary_path(&exe)).unwrap(),
            b"current",
            "the previous binary should be preserved for rollback"
        );
    }

    #[test]
    fn matching_manifest_installs_as_verified() {
        let bytes = b"new-bytes";
        let manifest = format!("{}  residuum-linux-x86_64\n", sha256_hex(bytes));
        let decision = verify_download(Some(&manifest), "residuum-linux-x86_64", bytes);

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("residuum");
        std::fs::write(&exe, b"current").unwrap();

        let installed = install_verified_bytes(&exe, bytes, decision).unwrap();
        assert_eq!(installed.verification, ReleaseVerification::Verified);
        assert_eq!(std::fs::read(&exe).unwrap(), bytes);
    }

    #[test]
    fn missing_checksum_asset_is_a_missing_manifest() {
        let manifest =
            manifest_from_response(reqwest::StatusCode::NOT_FOUND, String::new()).unwrap();
        let decision = verify_download(manifest.as_deref(), "residuum-linux-x86_64", b"bytes");
        assert_eq!(decision.unwrap(), ReleaseVerification::Unverified);
    }

    #[test]
    fn checksum_download_failure_refuses_rather_than_installing_unverified() {
        let err = manifest_from_response(reqwest::StatusCode::BAD_GATEWAY, "down".to_string())
            .unwrap_err();
        assert!(
            rejection_message(&err).is_some(),
            "a checksum request that isn't a 404 should refuse the install"
        );

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("residuum");
        std::fs::write(&exe, b"current").unwrap();
        let refused = install_verified_bytes(&exe, b"new", Err(err)).unwrap_err();
        assert!(rejection_message(&refused).is_some());
        assert_eq!(std::fs::read(&exe).unwrap(), b"current");
    }

    #[test]
    fn manifest_that_omits_the_asset_or_is_unreadable_refuses_install() {
        let other = format!("{}  residuum-linux-aarch64\n", sha256_hex(b"other"));
        let omitted = verify_download(Some(&other), "residuum-linux-x86_64", b"bytes").unwrap_err();
        assert!(
            rejection_message(&omitted).is_some_and(|message| message.contains("doesn't include")),
            "a manifest that doesn't list this binary should refuse the install"
        );

        let unreadable =
            verify_download(Some("not a manifest\n"), "residuum-linux-x86_64", b"bytes")
                .unwrap_err();
        assert!(
            rejection_message(&unreadable).is_some_and(|message| message.contains("unreadable")),
            "a manifest with no checksum lines should refuse the install"
        );
    }

    #[test]
    fn unverified_update_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_unverified_update(dir.path()).is_none());

        let notice = UnverifiedUpdate {
            version: "v2026.03.02".to_string(),
            at: Utc::now(),
        };
        write_unverified_update(dir.path(), &notice);
        assert_eq!(read_unverified_update(dir.path()), Some(notice));

        clear_unverified_update(dir.path());
        assert!(read_unverified_update(dir.path()).is_none());
    }

    #[test]
    fn rollback_notice_clears_an_unverified_update_note() {
        let dir = tempfile::tempdir().unwrap();
        write_unverified_update(
            dir.path(),
            &UnverifiedUpdate {
                version: "v2026.09.24".to_string(),
                at: Utc::now(),
            },
        );
        write_rollback_notice(
            dir.path(),
            &RollbackNotice {
                attempted_version: "v2026.09.24".to_string(),
                reason: "did not become healthy within 60 seconds".to_string(),
                at: Utc::now(),
            },
        );
        assert!(
            read_unverified_update(dir.path()).is_none(),
            "a rollback should drop the unverified note for the version that is no longer running"
        );
    }
}
