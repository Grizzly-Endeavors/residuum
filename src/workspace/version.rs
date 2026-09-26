//! Version tokens for workspace files.
//!
//! A version token is an opaque string derived from a file's modification
//! time (nanosecond precision where the platform stores it) and size. Any
//! write that changes either produces a new token. Listings, reads, and
//! writes all derive it through this one function so they never disagree.

use std::time::UNIX_EPOCH;

/// Compute a workspace file's version token from its metadata.
///
/// Callers must treat the result as opaque — it has no meaning beyond
/// equality comparison, and its format may change.
#[must_use]
pub fn version_token(metadata: &std::fs::Metadata) -> String {
    let (secs, nanos) = modified_since_epoch(metadata).unwrap_or((0, 0));
    format!("{secs:x}-{nanos:x}-{:x}", metadata.len())
}

/// The file's modification time in Unix milliseconds, `0` if the platform
/// can't report one.
#[must_use]
pub fn modified_unix_ms(metadata: &std::fs::Metadata) -> u64 {
    let Some((secs, nanos)) = modified_since_epoch(metadata) else {
        return 0;
    };
    let millis = u128::from(secs) * 1000 + u128::from(nanos) / 1_000_000;
    u64::try_from(millis).unwrap_or(u64::MAX)
}

/// Seconds and nanoseconds since the Unix epoch for a file's modification
/// time, or `None` if the platform doesn't report one.
fn modified_since_epoch(metadata: &std::fs::Metadata) -> Option<(u64, u32)> {
    let duration = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((duration.as_secs(), duration.subsec_nanos()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_changes_when_content_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");

        std::fs::write(&path, "one").unwrap();
        let v1 = version_token(&std::fs::metadata(&path).unwrap());

        // Force a distinguishable mtime, then change the size too.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, "two-longer").unwrap();
        let v2 = version_token(&std::fs::metadata(&path).unwrap());

        assert_ne!(v1, v2, "version token should change after a write");
    }

    #[test]
    fn modified_unix_ms_is_nonzero_for_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "hello").unwrap();

        let ms = modified_unix_ms(&std::fs::metadata(&path).unwrap());
        assert!(ms > 0, "a freshly written file should have a nonzero mtime");
    }
}
