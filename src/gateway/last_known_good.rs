//! Last-known-good config: after startup or a reload succeeds, `config.toml`
//! and `providers.toml` are copied here. If a later startup hits a fatal
//! config problem — the live files fail to load, or load but can't produce
//! a working gateway (no usable main provider, etc.) — the gateway falls
//! back to running on these copies instead of refusing to start, without
//! touching the user's live files. Replaces the old `.bak` mechanism, which
//! refreshed on every load attempt before anything proved the config
//! actually worked, so a bad reload could overwrite a good backup with
//! itself.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::util::FatalError;

fn config_copy_path(config_dir: &Path) -> PathBuf {
    config_dir.join("config.last-known-good.toml")
}

fn providers_copy_path(config_dir: &Path) -> PathBuf {
    config_dir.join("providers.last-known-good.toml")
}

/// Save `config.toml` and `providers.toml` as the last-known-good copies.
///
/// Call only once the gateway has actually started successfully on them,
/// or a reload has applied them successfully — that's what "known good"
/// means here. Each copy is written atomically (write to a temp file, then
/// rename over the old copy) so a crash mid-write can never leave a
/// half-written copy for a later fallback to load. Best-effort: a copy
/// failure is logged, not fatal, and leaves the previous last-known-good
/// copy (if any) in place.
pub(crate) fn save(config_dir: &Path) {
    for (src_name, dst_path) in [
        ("config.toml", config_copy_path(config_dir)),
        ("providers.toml", providers_copy_path(config_dir)),
    ] {
        let src = config_dir.join(src_name);
        if !src.exists() {
            continue;
        }
        if let Err(err) = atomic_copy(&src, &dst_path) {
            tracing::warn!(
                file = src_name,
                error = %err,
                "failed to save last-known-good config copy"
            );
        }
    }
}

/// Copy `src` to `dst` via a temp file in the same directory plus a rename,
/// so the copy is atomic from any other reader's point of view.
fn atomic_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    let tmp = dst.with_extension("tmp");
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dst)
}

/// Whether a last-known-good pair is saved for `config_dir`.
#[must_use]
pub fn exists(config_dir: &Path) -> bool {
    config_copy_path(config_dir).exists() && providers_copy_path(config_dir).exists()
}

/// Load `Config` from the last-known-good copies, without touching the
/// user's live `config.toml`/`providers.toml`.
///
/// # Errors
/// Returns `FatalError::Config` if no last-known-good pair is saved, or the
/// saved copies themselves fail to load — the latter shouldn't happen,
/// since they were only ever saved right after a successful start.
pub(crate) fn load(config_dir: &Path) -> Result<Config, FatalError> {
    if !exists(config_dir) {
        return Err(FatalError::Config(
            "no last-known-good configuration is saved yet".to_string(),
        ));
    }
    Config::load_from_paths(
        config_dir,
        &config_copy_path(config_dir),
        &providers_copy_path(config_dir),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONFIG: &str = "timezone = \"UTC\"\n";
    const VALID_PROVIDERS: &str = "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";

    #[test]
    fn no_copy_saved_reports_not_existing_and_fails_to_load() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!exists(dir.path()));
        assert!(load(dir.path()).is_err());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), VALID_CONFIG).unwrap();
        std::fs::write(dir.path().join("providers.toml"), VALID_PROVIDERS).unwrap();

        save(dir.path());
        assert!(exists(dir.path()));

        let cfg = load(dir.path()).expect("saved last-known-good copy should load");
        assert_eq!(cfg.timezone, chrono_tz::UTC);
        assert_eq!(
            cfg.main.first().map(|p| p.model.model.as_str()),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn save_never_touches_the_live_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), VALID_CONFIG).unwrap();
        std::fs::write(dir.path().join("providers.toml"), VALID_PROVIDERS).unwrap();
        save(dir.path());

        // Now break the live config; the saved copy must stay untouched.
        std::fs::write(dir.path().join("config.toml"), "not valid toml [[[").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
            "not valid toml [[["
        );
        let cfg = load(dir.path()).expect("last-known-good copy should still load");
        assert_eq!(cfg.timezone, chrono_tz::UTC);
    }

    #[test]
    fn save_is_a_noop_when_nothing_has_loaded_yet() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path());
        assert!(!exists(dir.path()), "no source files, nothing to save");
    }

    #[test]
    fn later_save_overwrites_the_previous_copy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), VALID_CONFIG).unwrap();
        std::fs::write(dir.path().join("providers.toml"), VALID_PROVIDERS).unwrap();
        save(dir.path());

        let updated_providers = "[models]\nmain = \"anthropic/claude-opus-4-6\"\n".to_string();
        std::fs::write(dir.path().join("providers.toml"), &updated_providers).unwrap();
        save(dir.path());

        let cfg = load(dir.path()).unwrap();
        assert_eq!(
            cfg.main.first().map(|p| p.model.model.as_str()),
            Some("claude-opus-4-6"),
            "the newer save should replace the old last-known-good copy"
        );
    }
}
