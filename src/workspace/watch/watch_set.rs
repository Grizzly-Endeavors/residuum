//! A WebSocket connection's watched path prefixes, and which changes they
//! match.

use super::WorkspaceChange;

/// A batch with more matching changes than this reaches a connection as a
/// resync instead of a change list.
pub const MAX_CHANGES_PER_FRAME: usize = 500;

/// Why a `watch_workspace` request was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidWatchPrefix {
    #[error(
        "can't watch {0:?}: watch paths are relative to the workspace, like \"wiki\" or \"\" for everything"
    )]
    Absolute(String),
    #[error("can't watch {0:?}: watch paths must stay inside the workspace (no \"..\")")]
    LeavesWorkspace(String),
    #[error("can't watch {0:?}: it contains a character that can't appear in a workspace path")]
    InvalidCharacter(String),
}

/// The prefixes one connection watches. Empty means not watching.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchSet {
    /// Normalized: `/`-separated, no empty or `.` segments. `""` is the
    /// whole workspace.
    prefixes: Vec<String>,
}

/// The part of a batch one connection receives.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchedChanges {
    /// No change matched.
    None,
    /// The matching changes, in batch order.
    Changes(Vec<WorkspaceChange>),
    /// More than [`MAX_CHANGES_PER_FRAME`] changes matched.
    TooMany,
}

impl WatchSet {
    /// Validate and normalize the prefixes of a `watch_workspace` request.
    ///
    /// There is no cap on prefix count or length: a connection watching an
    /// unreasonable number of paths, or a very long one, just costs more to
    /// match against each batch — and a batch that matches too much already
    /// degrades to a resync (see [`MAX_CHANGES_PER_FRAME`]) rather than
    /// failing.
    ///
    /// # Errors
    /// Returns [`InvalidWatchPrefix`] for an absolute path, a `..` segment,
    /// or a backslash or NUL.
    pub fn parse(prefixes: Vec<String>) -> Result<Self, InvalidWatchPrefix> {
        let mut normalized = Vec::with_capacity(prefixes.len());
        for prefix in prefixes {
            let prefix = normalize_prefix(prefix)?;
            if !normalized.contains(&prefix) {
                normalized.push(prefix);
            }
        }
        Ok(Self {
            prefixes: normalized,
        })
    }

    /// Whether this connection watches nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty()
    }

    /// Whether a change at `path` concerns a watched prefix: the prefix
    /// itself, anything under it (by whole segments, so `wiki` never matches
    /// `wikipedia`), or a directory containing it, since creating, removing,
    /// or renaming that directory carries the prefix with it.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        self.prefixes
            .iter()
            .any(|prefix| is_within(path, prefix) || is_within(prefix, path))
    }

    /// The changes of one batch this connection receives.
    #[must_use]
    pub fn filter(&self, changes: &[WorkspaceChange]) -> WatchedChanges {
        if self.is_empty() {
            return WatchedChanges::None;
        }
        let mut matched = Vec::new();
        for change in changes.iter().filter(|c| self.matches(&c.path)) {
            if matched.len() == MAX_CHANGES_PER_FRAME {
                return WatchedChanges::TooMany;
            }
            matched.push(change.clone());
        }
        if matched.is_empty() {
            WatchedChanges::None
        } else {
            WatchedChanges::Changes(matched)
        }
    }
}

/// Whether `path` is `ancestor` or lies under it, by whole segments. The
/// empty path is the workspace root and contains everything.
fn is_within(path: &str, ancestor: &str) -> bool {
    ancestor.is_empty()
        || path
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn normalize_prefix(prefix: String) -> Result<String, InvalidWatchPrefix> {
    if prefix.contains(['\\', '\0']) {
        return Err(InvalidWatchPrefix::InvalidCharacter(prefix));
    }
    let first = prefix.split('/').next().unwrap_or_default();
    // A leading `/` or a drive letter (`C:`) names a path outside the workspace.
    if prefix.starts_with('/') || first.contains(':') {
        return Err(InvalidWatchPrefix::Absolute(prefix));
    }
    let mut segments = Vec::new();
    for segment in prefix.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err(InvalidWatchPrefix::LeavesWorkspace(prefix)),
            other => segments.push(other),
        }
    }
    Ok(segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::super::WorkspaceChangeKind;
    use super::*;

    fn set(prefixes: &[&str]) -> WatchSet {
        WatchSet::parse(prefixes.iter().map(ToString::to_string).collect()).unwrap()
    }

    fn change(path: &str) -> WorkspaceChange {
        WorkspaceChange {
            path: path.to_string(),
            kind: WorkspaceChangeKind::Modified,
        }
    }

    #[test]
    fn prefixes_match_by_whole_segments() {
        let watch = set(&["wiki"]);
        assert!(watch.matches("wiki"));
        assert!(watch.matches("wiki/a.md"));
        assert!(watch.matches("wiki/deep/b.md"));
        assert!(!watch.matches("wikipedia/a.md"));
        assert!(!watch.matches("wiki.md"));
        assert!(!watch.matches("notes/wiki/a.md"));
    }

    #[test]
    fn a_file_prefix_matches_only_that_file() {
        let watch = set(&["inbox/user/today.md"]);
        assert!(watch.matches("inbox/user/today.md"));
        assert!(!watch.matches("inbox/user/today.md.bak"));
        assert!(!watch.matches("inbox/user/other.md"));
    }

    #[test]
    fn a_change_to_a_containing_directory_matches() {
        let watch = set(&["projects/alpha/notes"]);
        assert!(watch.matches("projects/alpha"));
        assert!(watch.matches("projects"));
        assert!(!watch.matches("projects/beta"));
        assert!(!watch.matches("projects/alph"));
    }

    #[test]
    fn the_empty_prefix_is_the_whole_workspace() {
        let watch = set(&[""]);
        assert!(watch.matches("anything/at/all.md"));
        assert!(watch.matches("SOUL.md"));
    }

    #[test]
    fn prefixes_are_normalized() {
        assert_eq!(
            set(&["wiki/", "./wiki", "wiki//sub/."]),
            set(&["wiki", "wiki/sub"])
        );
    }

    #[test]
    fn invalid_prefixes_are_refused() {
        for (prefix, expected) in [
            ("../secrets", "LeavesWorkspace"),
            ("wiki/../../etc", "LeavesWorkspace"),
            ("/etc", "Absolute"),
            ("C:/Windows", "Absolute"),
            ("wiki\\a", "InvalidCharacter"),
        ] {
            let err = WatchSet::parse(vec![prefix.to_string()]).unwrap_err();
            assert!(
                format!("{err:?}").starts_with(expected),
                "{prefix}: {err:?}"
            );
        }
    }

    #[test]
    fn there_is_no_cap_on_prefix_count_or_length() {
        let many: Vec<String> = (0..500).map(|i| format!("p{i}")).collect();
        assert!(
            WatchSet::parse(many).is_ok(),
            "any number of prefixes should be accepted"
        );

        let long_prefix = vec!["a/".repeat(1000)];
        assert!(
            WatchSet::parse(long_prefix).is_ok(),
            "a long prefix should be accepted"
        );
    }

    #[test]
    fn an_empty_set_receives_nothing() {
        assert_eq!(
            WatchSet::default().filter(&[change("wiki/a.md")]),
            WatchedChanges::None
        );
        assert!(set(&[]).is_empty());
    }

    #[test]
    fn filter_keeps_only_matching_changes() {
        let watch = set(&["wiki", "inbox/user"]);
        let batch = [
            change("inbox/user/x.md"),
            change("wiki/a.md"),
            change("wikipedia/b.md"),
            change("memory/obs.json"),
        ];
        assert_eq!(
            watch.filter(&batch),
            WatchedChanges::Changes(vec![change("inbox/user/x.md"), change("wiki/a.md")])
        );
        assert_eq!(set(&["notes"]).filter(&batch), WatchedChanges::None);
    }

    #[test]
    fn more_than_the_frame_limit_becomes_too_many() {
        let at_limit: Vec<_> = (0..MAX_CHANGES_PER_FRAME)
            .map(|i| change(&format!("wiki/{i}.md")))
            .collect();
        assert!(matches!(
            set(&["wiki"]).filter(&at_limit),
            WatchedChanges::Changes(c) if c.len() == MAX_CHANGES_PER_FRAME
        ));

        let mut over = at_limit;
        over.push(change("wiki/one-more.md"));
        assert_eq!(set(&["wiki"]).filter(&over), WatchedChanges::TooMany);
        // Changes outside the prefixes don't count toward the limit.
        assert!(matches!(
            set(&["wiki/1.md"]).filter(&over),
            WatchedChanges::Changes(c) if c.len() == 1
        ));
    }
}
