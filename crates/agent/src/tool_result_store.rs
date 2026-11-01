//! Tool results too long to keep in history.
//!
//! Every tool result stays in history and is sent again with every later
//! model call, so one huge result (a long MCP response, a big `exec` dump)
//! would make every request after it larger. The turn loop hands each
//! redacted result to [`ToolResultStore::keep_for_history`]: a result over
//! [`OFFLOAD_THRESHOLD_CHARS`] is saved to a file, and history keeps its
//! head and tail with a note naming the file. The note is computed once and
//! stored as the tool message, so replaying history sends the same bytes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::io::AsyncWriteExt;

/// A tool result longer than this many characters is saved to a file and
/// kept in history as an excerpt. About 8k tokens of English text.
pub(crate) const OFFLOAD_THRESHOLD_CHARS: usize = 32_000;

/// Characters of the result's start kept in history.
const HEAD_CHARS: usize = 8_000;

/// Characters of the result's end kept in history, where errors and
/// summaries usually are.
const TAIL_CHARS: usize = 4_000;

/// How long a saved result is kept before [`ToolResultStore::sweep_expired`]
/// deletes it.
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Where an agent's oversized tool results are saved.
#[derive(Debug, Clone)]
pub struct ToolResultStore {
    /// The directory every agent's results live under (`hub/tool-results`).
    root: PathBuf,
    /// This agent's own directory under `root`.
    dir: PathBuf,
}

impl ToolResultStore {
    /// A store saving into `root/<agent>`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, agent: &str) -> Self {
        let root = root.into();
        let dir = root.join(agent);
        Self { root, dir }
    }

    /// This agent's directory of saved results.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What history keeps for `output`: `output` itself when it is short
    /// enough, otherwise its head and tail around a note naming the file the
    /// full text was saved to.
    ///
    /// When the file can't be written, the full output is kept, so nothing
    /// the tool returned is lost; the failure is logged.
    pub(crate) async fn keep_for_history(
        &self,
        output: String,
        tool_name: &str,
        tool_call_id: &str,
    ) -> String {
        if output.len() <= OFFLOAD_THRESHOLD_CHARS
            || output.chars().count() <= OFFLOAD_THRESHOLD_CHARS
        {
            return output;
        }
        match self.save(&output, tool_name, tool_call_id).await {
            Ok(path) => {
                tracing::debug!(
                    tool_name,
                    tool_call_id,
                    chars = output.chars().count(),
                    path = %path.display(),
                    "saved an oversized tool result to a file and kept an excerpt in history"
                );
                excerpt(&output, &path)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    tool_name,
                    tool_call_id,
                    dir = %self.dir.display(),
                    "failed to save an oversized tool result; keeping the full result in history"
                );
                output
            }
        }
    }

    async fn save(
        &self,
        output: &str,
        tool_name: &str,
        tool_call_id: &str,
    ) -> std::io::Result<PathBuf> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let path = self.dir.join(file_name(tool_name, tool_call_id));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        file.write_all(output.as_bytes()).await?;
        file.flush().await?;
        Ok(path)
    }

    /// Delete saved results older than seven days, for every agent under
    /// the store's root (including agents since deleted). Returns how many
    /// files were removed.
    pub async fn sweep_expired(&self) -> usize {
        let cutoff = SystemTime::now()
            .checked_sub(RETENTION)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.sweep_before(cutoff).await
    }

    /// Delete saved results last modified before `cutoff`: files directly
    /// under the root and in each agent directory under it.
    async fn sweep_before(&self, cutoff: SystemTime) -> usize {
        let mut removed = 0;
        let mut agent_dirs = Vec::new();
        for entry in list_dir(&self.root).await {
            match entry.file_type().await {
                Ok(kind) if kind.is_dir() => agent_dirs.push(entry.path()),
                Ok(kind) if kind.is_file() => {
                    removed += usize::from(remove_if_expired(&entry.path(), cutoff).await);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    error = %e,
                    path = %entry.path().display(),
                    "failed to inspect a saved tool result entry"
                ),
            }
        }
        for dir in agent_dirs {
            for entry in list_dir(&dir).await {
                if entry.file_type().await.is_ok_and(|kind| kind.is_file()) {
                    removed += usize::from(remove_if_expired(&entry.path(), cutoff).await);
                }
            }
        }
        if removed > 0 {
            tracing::debug!(
                removed,
                root = %self.root.display(),
                "deleted expired saved tool results"
            );
        }
        removed
    }
}

/// Every entry of `dir`; empty when it doesn't exist (nothing saved yet).
async fn list_dir(dir: &Path) -> Vec<tokio::fs::DirEntry> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "failed to list saved tool results");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    loop {
        match entries.next_entry().await {
            Ok(Some(entry)) => out.push(entry),
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, dir = %dir.display(), "failed to list saved tool results");
                break;
            }
        }
    }
    out
}

/// Remove `path` if it was last modified before `cutoff`. A file another
/// sweep already removed counts as not removed here.
async fn remove_if_expired(path: &Path, cutoff: SystemTime) -> bool {
    let modified = match tokio::fs::metadata(path).await.and_then(|m| m.modified()) {
        Ok(modified) => modified,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "failed to read a saved tool result's age");
            return false;
        }
    };
    if modified >= cutoff {
        return false;
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "failed to delete an expired saved tool result");
            false
        }
    }
}

/// A unique, filesystem-safe name: the time saved, then the tool and call id
/// so a person browsing the directory can tell results apart.
fn file_name(tool_name: &str, tool_call_id: &str) -> String {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ");
    format!(
        "{stamp}-{}-{}.txt",
        file_name_part(tool_name),
        file_name_part(tool_call_id)
    )
}

/// `part` reduced to ASCII letters, digits, `-` and `_`, at most 48 of them.
fn file_name_part(part: &str) -> String {
    part.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(48)
        .collect()
}

/// The head and tail of `output` around a note naming `path`, the omitted
/// line range, and how to read it.
fn excerpt(output: &str, path: &Path) -> String {
    let head_end = head_end(output);
    let tail_start = tail_start(output).max(head_end);
    let head = output.get(..head_end).unwrap_or_default();
    let tail = output.get(tail_start..).unwrap_or_default();
    let before_tail = output.get(..tail_start).unwrap_or_default();

    let total_chars = output.chars().count();
    let total_lines = output.lines().count();
    let first_omitted = head.matches('\n').count() + 1;
    let newlines_before_tail = before_tail.matches('\n').count();
    let last_omitted = if before_tail.ends_with('\n') {
        newlines_before_tail
    } else {
        newlines_before_tail + 1
    };
    let omitted = if first_omitted >= last_omitted {
        format!("Part of line {first_omitted} is")
    } else {
        format!("Lines {first_omitted}-{last_omitted} are")
    };
    let offset = first_omitted.saturating_sub(1);
    let limit = last_omitted.saturating_sub(first_omitted) + 1;

    let note = format!(
        "[Output too long to keep in full: {total_chars} characters, {total_lines} lines. \
         The full output is saved at {}. {omitted} left out here; read them with read_file \
         (offset {offset}, limit {limit}), or search the file with exec.]",
        path.display()
    );
    format!("{}\n\n{note}\n\n{tail}", head.trim_end_matches('\n'))
}

/// Byte index the head ends at: after [`HEAD_CHARS`] characters, moved back
/// to the end of the last whole line when one ends in the second half.
fn head_end(output: &str) -> usize {
    let cut = output
        .char_indices()
        .nth(HEAD_CHARS)
        .map_or(output.len(), |(i, _)| i);
    let head = output.get(..cut).unwrap_or_default();
    match head.rfind('\n') {
        Some(nl) if nl >= cut / 2 => nl + 1,
        _ => cut,
    }
}

/// Byte index the tail starts at: [`TAIL_CHARS`] characters before the end,
/// moved forward to the start of the next whole line when one starts in the
/// first half.
fn tail_start(output: &str) -> usize {
    let cut = output
        .char_indices()
        .rev()
        .nth(TAIL_CHARS - 1)
        .map_or(0, |(i, _)| i);
    let tail = output.get(cut..).unwrap_or_default();
    match tail.find('\n') {
        Some(nl) if nl < tail.len() / 2 => cut + nl + 1,
        _ => cut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered_lines(count: usize) -> String {
        (1..=count).map(|n| format!("line {n}\n")).collect()
    }

    fn saved_files(dir: &Path) -> Vec<PathBuf> {
        match std::fs::read_dir(dir) {
            Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
            Err(_) => Vec::new(),
        }
    }

    #[tokio::test]
    async fn a_result_at_the_threshold_is_kept_whole() {
        let root = tempfile::tempdir().unwrap();
        let store = ToolResultStore::new(root.path(), "scout");
        let output = "x".repeat(OFFLOAD_THRESHOLD_CHARS);

        let kept = store
            .keep_for_history(output.clone(), "exec", "call-1")
            .await;

        assert_eq!(kept, output, "a result at the threshold is not touched");
        assert!(saved_files(store.dir()).is_empty(), "nothing is saved");
    }

    #[tokio::test]
    async fn the_threshold_counts_characters_not_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = ToolResultStore::new(root.path(), "scout");
        // Three bytes per character: over the threshold in bytes, under it in
        // characters.
        let output = "€".repeat(OFFLOAD_THRESHOLD_CHARS);

        let kept = store
            .keep_for_history(output.clone(), "exec", "call-1")
            .await;

        assert_eq!(kept, output);
        assert!(saved_files(store.dir()).is_empty());
    }

    #[tokio::test]
    async fn an_oversized_result_is_saved_whole_and_excerpted_by_line() {
        let root = tempfile::tempdir().unwrap();
        let store = ToolResultStore::new(root.path(), "scout");
        let output = numbered_lines(10_000);

        let kept = store
            .keep_for_history(output.clone(), "mcp tool/odd:name", "call:1")
            .await;

        let files = saved_files(store.dir());
        assert_eq!(files.len(), 1, "one file per offloaded result: {files:?}");
        let file = files.first().unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), output);
        let name = file.file_name().unwrap().to_str().unwrap();
        assert!(
            name.ends_with("-mcptooloddname-call1.txt"),
            "tool name and call id are reduced to safe characters: {name}"
        );

        assert!(
            kept.chars().count() < HEAD_CHARS + TAIL_CHARS + 1_000,
            "{}",
            kept.len()
        );
        assert!(kept.starts_with("line 1\nline 2\n"), "head is kept");
        assert!(kept.ends_with("line 10000\n"), "tail is kept");
        assert!(
            kept.contains(&format!("saved at {}", file.display())),
            "the note names the file: {kept}"
        );
        assert!(
            kept.contains("10000 lines"),
            "the note gives the size: {kept}"
        );

        // The note's line range matches what the head and tail hold, and the
        // read_file offset it suggests starts right after the head.
        let (head, rest) = kept.split_once("\n\n[").unwrap();
        let (note, tail) = rest.split_once("]\n\n").unwrap();
        let last_head_line: usize = head
            .lines()
            .last()
            .unwrap()
            .trim_start_matches("line ")
            .parse()
            .unwrap();
        let first_tail_line: usize = tail
            .lines()
            .next()
            .unwrap()
            .trim_start_matches("line ")
            .parse()
            .unwrap();
        let expected = format!(
            "Lines {}-{} are left out here; read them with read_file (offset {}, limit {})",
            last_head_line + 1,
            first_tail_line - 1,
            last_head_line,
            first_tail_line - 1 - last_head_line,
        );
        assert!(
            note.contains(&expected),
            "expected {expected:?} in {note:?}"
        );
    }

    #[test]
    fn a_single_long_line_is_cut_by_characters() {
        let output = format!("START{}END", "x".repeat(50_000));
        let kept = excerpt(&output, Path::new("/hub/tool-results/scout/r.txt"));

        assert!(kept.starts_with("START"));
        assert!(kept.ends_with("END"));
        assert!(kept.contains("Part of line 1 is left out here"), "{kept}");
        assert!(kept.contains("50008 characters, 1 lines"), "{kept}");
    }

    #[test]
    fn excerpt_cuts_on_character_boundaries() {
        let output = "é".repeat(40_000);
        let kept = excerpt(&output, Path::new("/r.txt"));
        assert!(kept.starts_with('é') && kept.ends_with('é'));
    }

    #[tokio::test]
    async fn sweep_deletes_results_older_than_the_retention_for_every_agent() {
        let root = tempfile::tempdir().unwrap();
        let store = ToolResultStore::new(root.path(), "scout");
        let other_agent = root.path().join("deleted-agent");
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::create_dir_all(&other_agent).unwrap();

        let now = SystemTime::now();
        let expired = now - RETENTION - Duration::from_secs(60);
        let write = |path: &Path, modified: SystemTime| {
            std::fs::write(path, "result").unwrap();
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        };
        let old_own = store.dir().join("old.txt");
        let fresh_own = store.dir().join("fresh.txt");
        let old_other = other_agent.join("old.txt");
        write(&old_own, expired);
        write(&fresh_own, now);
        write(&old_other, expired);

        let removed = store.sweep_before(now - RETENTION).await;

        assert_eq!(removed, 2);
        assert!(!old_own.exists(), "this agent's expired result is deleted");
        assert!(
            !old_other.exists(),
            "a deleted agent's expired result is deleted"
        );
        assert!(fresh_own.exists(), "a result inside the retention is kept");
    }

    #[tokio::test]
    async fn sweep_with_nothing_saved_yet_removes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let store = ToolResultStore::new(root.path().join("tool-results"), "scout");
        assert_eq!(store.sweep_expired().await, 0);
    }
}
