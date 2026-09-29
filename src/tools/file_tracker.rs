//! Shared read-tracking state for file tools.
//!
//! Tracks which files the agent has read so that write and edit tools can
//! enforce read-before-modify semantics, and, for team files, the stamp each
//! had when it was read so a write can detect a change made since.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::workspace::team_files::FileStamp;

/// Shared reference to a `FileTracker` behind an async mutex.
pub type SharedFileTracker = Arc<tokio::sync::Mutex<FileTracker>>;

/// Tracks which file paths the agent has previously read, and the stamp of
/// each team file at the moment it was read (see
/// [`crate::workspace::team_files::TeamWriteCoordinator`]).
pub struct FileTracker {
    read_paths: HashSet<PathBuf>,
    team_stamps: HashMap<PathBuf, FileStamp>,
}

impl FileTracker {
    /// Create a new, empty file tracker.
    #[must_use]
    fn new() -> Self {
        Self {
            read_paths: HashSet::new(),
            team_stamps: HashMap::new(),
        }
    }

    /// Create a new tracker wrapped in `Arc<tokio::sync::Mutex<_>>`.
    #[must_use]
    pub fn new_shared() -> SharedFileTracker {
        Arc::new(tokio::sync::Mutex::new(Self::new()))
    }

    /// Canonical form of `path`, or the raw path when it can't be resolved.
    fn key(path: &str) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| {
            tracing::trace!(path = %path, "canonicalize failed, using raw path");
            PathBuf::from(path)
        })
    }

    /// Record that a file has been read. Canonicalizes the path where possible.
    pub fn record_read(&mut self, path: &str) {
        self.read_paths.insert(Self::key(path));
    }

    /// Record that a team file has been read or written, with its stamp from
    /// just before the read or right after the write. `None` (the stamp could
    /// not be taken) makes any later write to the file report a conflict
    /// rather than pass unchecked.
    pub fn record_team_read(&mut self, path: &str, stamp: Option<FileStamp>) {
        self.record_read(path);
        let key = Self::key(path);
        match stamp {
            Some(stamp) => {
                self.team_stamps.insert(key, stamp);
            }
            None => {
                self.team_stamps.remove(&key);
            }
        }
    }

    /// The stamp a team file had when this tracker's tools last read or wrote
    /// it.
    #[must_use]
    pub fn team_stamp(&self, path: &str) -> Option<FileStamp> {
        self.team_stamps.get(&Self::key(path)).cloned()
    }

    /// Check whether a file has been previously read.
    #[must_use]
    pub fn has_been_read(&self, path: &str) -> bool {
        self.read_paths.contains(&Self::key(path))
    }
}

#[cfg(test)]
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

    #[test]
    fn team_stamp_round_trips_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("team_file.md");
        std::fs::write(&file_path, "data").unwrap();
        let path = file_path.to_str().unwrap();
        let stamp = FileStamp {
            version: Some("v".to_string()),
            generation: 3,
        };

        let mut tracker = FileTracker::new();
        tracker.record_team_read(path, Some(stamp.clone()));
        assert!(tracker.has_been_read(path));
        assert_eq!(tracker.team_stamp(path), Some(stamp));

        tracker.record_team_read(path, None);
        assert_eq!(tracker.team_stamp(path), None);
    }
}
