//! Shared read-tracking state for file tools.
//!
//! Tracks which files the agent has read so that write and edit tools can
//! enforce read-before-modify semantics.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Shared reference to a `FileTracker` behind an async mutex.
pub type SharedFileTracker = Arc<tokio::sync::Mutex<FileTracker>>;

/// Tracks which file paths the agent has previously read.
pub struct FileTracker {
    read_paths: HashSet<PathBuf>,
}

impl FileTracker {
    /// Create a new, empty file tracker.
    #[must_use]
    fn new() -> Self {
        Self {
            read_paths: HashSet::new(),
        }
    }

    /// Create a new tracker wrapped in `Arc<tokio::sync::Mutex<_>>`.
    #[must_use]
    pub fn new_shared() -> SharedFileTracker {
        Arc::new(tokio::sync::Mutex::new(Self::new()))
    }

    /// Record that a file has been read. Canonicalizes the path where possible.
    pub fn record_read(&mut self, path: &str) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| {
            tracing::trace!(path = %path, "canonicalize failed, using raw path");
            PathBuf::from(path)
        });
        self.read_paths.insert(canonical);
    }

    /// Check whether a file has been previously read.
    #[must_use]
    pub fn has_been_read(&self, path: &str) -> bool {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| {
            tracing::trace!(path = %path, "canonicalize failed, using raw path");
            PathBuf::from(path)
        });
        self.read_paths.contains(&canonical)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn record_and_check() {
        let mut tracker = FileTracker::new();
        tracker.record_read("/tmp/test_file_tracker_a.txt");
        assert!(
            tracker.has_been_read("/tmp/test_file_tracker_a.txt"),
            "recorded path should be found"
        );
    }

    #[test]
    fn unread_returns_false() {
        let tracker = FileTracker::new();
        assert!(
            !tracker.has_been_read("/nonexistent/path.txt"),
            "unread path should return false"
        );
    }

    #[tokio::test]
    async fn canonicalization_equivalence() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("canon_test.txt");
        std::fs::write(&file_path, "data").unwrap();

        let mut tracker = FileTracker::new();
        // Record via absolute path
        tracker.record_read(file_path.to_str().unwrap());

        // Check via the same path — should be canonicalized identically
        assert!(
            tracker.has_been_read(file_path.to_str().unwrap()),
            "canonical path should match"
        );
    }
}
