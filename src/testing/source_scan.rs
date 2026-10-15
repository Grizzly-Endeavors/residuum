//! Rules checked by reading the workspace's own source: the root crate's and
//! every workspace member's `src` and `tests`.
//!
//! Both rules need to know which lines are test code, and a file is not split
//! cleanly at its first `#[cfg(test)]`: plenty of files put the attribute on a
//! `use` or an accessor near the top and carry on with production code. The
//! classifier here goes by module declarations and by the shape rustfmt gives
//! an item:
//! - A whole file is test code when it is under a `tests/` directory of a
//!   scanned root, or when a `#[cfg(test)] mod x;` (or a `mod x;` inside a test
//!   file) declares it.
//! - Elsewhere, a `#[cfg(test)]` or `#[cfg(any(test, feature = "test-support"))]`
//!   line covers the item after it (past any further attributes and comments).
//!   That item ends at its own line when the line ends in `;`, or at the first
//!   later line at the item's indentation that closes it (`}`, `)` or `]`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use regex::Regex;

use super::wall_clock_backlog::{ALLOWED, BACKLOG};

/// Whether `line` is an attribute that makes the item after it test code: a
/// `#[cfg(test)]`, or the `test-support` gate, which compiles the same helpers
/// into another crate's test build.
fn is_test_gate(line: &str) -> bool {
    matches!(
        line.trim(),
        "#[cfg(test)]" | r#"#[cfg(any(test, feature = "test-support"))]"#
    )
}

/// A source file, its lines, and which of them are test code.
struct SourceFile {
    /// Relative to the repository root, with `/` separators.
    path: String,
    lines: Vec<String>,
    test: Vec<bool>,
}

impl SourceFile {
    /// The file's lines with their 1-based numbers, test code or not, with
    /// comments removed.
    fn code_lines(&self, want_test: bool) -> impl Iterator<Item = (usize, &str)> {
        self.lines
            .iter()
            .zip(&self.test)
            .enumerate()
            .filter(move |(_, (_, test))| **test == want_test)
            .filter_map(|(idx, (line, _))| without_comment(line).map(|code| (idx + 1, code)))
    }
}

/// The code on `line`, without a `//` comment; `None` for a comment line.
fn without_comment(line: &str) -> Option<&str> {
    if line.trim_start().starts_with("//") {
        return None;
    }
    // A ` //` with an even number of quotes before it starts a comment, not
    // the inside of a string literal.
    let mut search_from = 0;
    while let Some(found) = line.get(search_from..).and_then(|rest| rest.find(" //")) {
        let at = search_from + found;
        let before = line.get(..at).unwrap_or_default();
        if before.matches('"').count() % 2 == 0 {
            return Some(before);
        }
        search_from = at + 3;
    }
    Some(line)
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `[workspace] members` the root manifest names; none when it has no
/// `[workspace]` table or no `members` key.
fn workspace_members(manifest: &toml::Table) -> Result<Vec<String>, String> {
    let Some(members) = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
    else {
        return Ok(Vec::new());
    };
    let Some(items) = members.as_array() else {
        return Err("[workspace] members must be an array of strings".to_string());
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| "[workspace] members must be an array of strings".to_string())
        })
        .collect()
}

/// The directories (relative to `root`) that `members` names. Only literal
/// paths and a trailing `/*` glob are supported; a `/*` glob matches each
/// directory under its prefix that holds a `Cargo.toml`.
fn member_dirs(root: &Path, members: &[String]) -> Result<Vec<String>, String> {
    let mut dirs = Vec::new();
    for member in members {
        if let Some(prefix) = member.strip_suffix("/*") {
            if prefix.contains(['*', '?', '[']) {
                return Err(format!(
                    "unsupported workspace member pattern {member:?}: only literal paths and trailing `/*` globs are supported"
                ));
            }
            let entries = std::fs::read_dir(root.join(prefix))
                .map_err(|err| format!("failed to list workspace member {member:?}: {err}"))?;
            let mut found = Vec::new();
            for entry in entries {
                let path = entry
                    .map_err(|err| format!("failed to list workspace member {member:?}: {err}"))?
                    .path();
                if path.join("Cargo.toml").is_file() {
                    found.push(relative(root, &path));
                }
            }
            found.sort();
            dirs.extend(found);
        } else if member.contains(['*', '?', '[']) {
            return Err(format!(
                "unsupported workspace member pattern {member:?}: only literal paths and trailing `/*` globs are supported"
            ));
        } else {
            dirs.push(member.clone());
        }
    }
    Ok(dirs)
}

/// The `src` and `tests` directories to scan, relative to the repository root:
/// the root's, then each workspace member's, keeping only those that exist.
fn scan_roots() -> Result<Vec<String>, String> {
    let root = crate_root();
    let manifest_path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|err| format!("failed to read {}: {err}", manifest_path.display()))?;
    let manifest: toml::Table = toml::from_str(&text)
        .map_err(|err| format!("failed to parse {}: {err}", manifest_path.display()))?;
    let members = workspace_members(&manifest)?;
    let mut crate_dirs = vec![String::new()];
    crate_dirs.extend(member_dirs(&root, &members)?);
    let mut scanned = Vec::new();
    for crate_dir in &crate_dirs {
        for leaf in ["src", "tests"] {
            let dir = if crate_dir.is_empty() {
                leaf.to_string()
            } else {
                format!("{crate_dir}/{leaf}")
            };
            if root.join(&dir).is_dir() {
                scanned.push(dir);
            }
        }
    }
    Ok(scanned)
}

fn rust_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files_under(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Leading spaces, the indentation rustfmt uses.
fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The file a `mod name;` in `declaring` refers to, if it exists.
fn module_file(declaring: &Path, name: &str) -> Option<PathBuf> {
    let parent = declaring.parent()?;
    let file_name = declaring.file_name()?.to_str()?;
    let dir = if matches!(file_name, "lib.rs" | "main.rs" | "mod.rs") {
        parent.to_path_buf()
    } else {
        parent.join(declaring.file_stem()?)
    };
    [
        dir.join(format!("{name}.rs")),
        dir.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// The index of the first line of the item that `#[cfg(test)]` at `at`
/// covers, past any further attributes, comments and blank lines.
fn item_start(lines: &[String], at: usize) -> usize {
    let mut idx = at + 1;
    while let Some(line) = lines.get(idx) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[") {
            // An attribute can span lines; skip to where its brackets close.
            let mut depth = 0_i32;
            while let Some(attr_line) = lines.get(idx) {
                depth += i32::try_from(attr_line.matches('[').count()).unwrap_or(0);
                depth -= i32::try_from(attr_line.matches(']').count()).unwrap_or(0);
                idx += 1;
                if depth <= 0 {
                    break;
                }
            }
        } else if trimmed.is_empty() || trimmed.starts_with("//") {
            idx += 1;
        } else {
            break;
        }
    }
    idx
}

/// Where the lexer stands at the end of a line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lexed {
    Code,
    /// Inside `"..."`; a trailing `\` escapes the newline, which changes nothing here.
    Str,
    /// Inside `r#"..."#` with this many `#`.
    RawStr(usize),
    /// Inside `/* ... */`, nested this deep.
    BlockComment(usize),
}

/// For each line, whether it starts inside a string literal or block comment.
/// Such a line's indentation is the literal's content, not rustfmt's, so a `}`
/// at column 0 in an embedded shell script must not end the item around it.
fn starts_inside_literal(lines: &[String]) -> Vec<bool> {
    let mut state = Lexed::Code;
    let mut inside = Vec::with_capacity(lines.len());
    for line in lines {
        inside.push(state != Lexed::Code);
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while let Some(&c) = chars.get(i) {
            let next = chars.get(i + 1).copied();
            match state {
                Lexed::Code => match c {
                    '/' if next == Some('/') => break,
                    '/' if next == Some('*') => {
                        state = Lexed::BlockComment(1);
                        i += 1;
                    }
                    '"' => state = Lexed::Str,
                    'r' if !i
                        .checked_sub(1)
                        .and_then(|p| chars.get(p))
                        .is_some_and(|p| p.is_alphanumeric() || *p == '_') =>
                    {
                        let hashes = chars
                            .iter()
                            .skip(i + 1)
                            .take_while(|ch| **ch == '#')
                            .count();
                        if chars.get(i + 1 + hashes) == Some(&'"') {
                            state = Lexed::RawStr(hashes);
                            i += 1 + hashes;
                        }
                    }
                    '\'' => {
                        // A char literal ('"', '\'', '\u{7f}'); a lifetime has no closing quote.
                        if next == Some('\\') {
                            if let Some(close) = chars.iter().skip(i + 2).position(|ch| *ch == '\'')
                            {
                                i += 2 + close;
                            }
                        } else if chars.get(i + 2) == Some(&'\'') {
                            i += 2;
                        }
                    }
                    _ => {}
                },
                Lexed::Str => match c {
                    '\\' => i += 1,
                    '"' => state = Lexed::Code,
                    _ => {}
                },
                Lexed::RawStr(hashes) => {
                    if c == '"'
                        && chars
                            .iter()
                            .skip(i + 1)
                            .take(hashes)
                            .filter(|ch| **ch == '#')
                            .count()
                            == hashes
                    {
                        state = Lexed::Code;
                        i += hashes;
                    }
                }
                Lexed::BlockComment(depth) => {
                    if c == '*' && next == Some('/') {
                        state = if depth == 1 {
                            Lexed::Code
                        } else {
                            Lexed::BlockComment(depth - 1)
                        };
                        i += 1;
                    } else if c == '/' && next == Some('*') {
                        state = Lexed::BlockComment(depth + 1);
                        i += 1;
                    }
                }
            }
            i += 1;
        }
    }
    inside
}

/// The index of the last line of the item starting at `start`.
fn item_end(lines: &[String], in_literal: &[bool], start: usize) -> usize {
    let Some(first) = lines.get(start) else {
        return start;
    };
    if first.trim_end().ends_with(';') {
        return start;
    }
    let item_indent = indent(first);
    for (idx, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || in_literal.get(idx).copied().unwrap_or(false) {
            continue;
        }
        let line_indent = indent(line);
        if line_indent < item_indent {
            return idx - 1;
        }
        if line_indent == item_indent {
            if trimmed.starts_with(['}', ')', ']']) {
                return idx;
            }
            if !trimmed.starts_with("where") && !trimmed.starts_with('{') {
                return idx - 1;
            }
        }
    }
    lines.len().saturating_sub(1)
}

/// Every Rust file in the scanned roots, classified.
fn crate_sources() -> Vec<SourceFile> {
    let root = crate_root();
    let mut paths = Vec::new();
    let mut test_dirs = Vec::new();
    for dir in scan_roots().expect("failed to read the workspace members") {
        let abs = root.join(&dir);
        rust_files_under(&abs, &mut paths);
        if abs.file_name().is_some_and(|name| name == "tests") {
            test_dirs.push(abs);
        }
    }
    paths.sort();
    let contents: BTreeMap<PathBuf, Vec<String>> = paths
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path).unwrap();
            (path.clone(), text.lines().map(str::to_string).collect())
        })
        .collect();

    let mod_decl = Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z0-9_]+)\s*;").unwrap();
    let mut test_files: BTreeSet<PathBuf> = paths
        .iter()
        .filter(|path| test_dirs.iter().any(|dir| path.starts_with(dir)))
        .cloned()
        .collect();
    // Declarations under `#[cfg(test)]`, then everything a test file
    // declares, until nothing new turns up.
    let mut pending: Vec<PathBuf> = Vec::new();
    for (path, lines) in &contents {
        for (idx, line) in lines.iter().enumerate() {
            if !is_test_gate(line) {
                continue;
            }
            let start = item_start(lines, idx);
            if let Some(name) = lines.get(start).and_then(|item| mod_decl.captures(item))
                && let Some(file) = module_file(path, &name[1])
                && test_files.insert(file.clone())
            {
                pending.push(file);
            }
        }
    }
    pending.extend(test_files.iter().cloned());
    while let Some(path) = pending.pop() {
        let Some(lines) = contents.get(&path) else {
            continue;
        };
        for line in lines {
            if let Some(name) = mod_decl.captures(line)
                && let Some(file) = module_file(&path, &name[1])
                && test_files.insert(file.clone())
            {
                pending.push(file);
            }
        }
    }

    contents
        .into_iter()
        .map(|(path, lines)| {
            let mut test = vec![test_files.contains(&path); lines.len()];
            if !test_files.contains(&path) {
                let in_literal = starts_inside_literal(&lines);
                let mut idx = 0;
                while idx < lines.len() {
                    if lines.get(idx).is_some_and(|line| is_test_gate(line)) {
                        let end = item_end(&lines, &in_literal, item_start(&lines, idx));
                        for flag in test.iter_mut().take(end + 1).skip(idx) {
                            *flag = true;
                        }
                        idx = end + 1;
                    } else {
                        idx += 1;
                    }
                }
            }
            SourceFile {
                path: relative(&root, &path),
                lines,
                test,
            }
        })
        .collect()
}

/// Files whose test code may wait on the clock: the waits that everything
/// else goes through.
const WAIT_HELPERS: &[&str] = &[
    "src/testing/wait.rs",
    "src/testing/clock.rs",
    "src/testing/source_scan.rs",
    // Integration tests can't reach `crate::testing`; these are its two waits
    // for them, under the same hang guard.
    "tests/it/support/until.rs",
    "tests/it/support/guarded.rs",
];

/// A wall-clock wait in test code: a sleep, a timeout, an import that brings
/// one in under a bare name, a bare call to one, or a wiremock reply delay.
fn wall_clock_wait() -> Regex {
    Regex::new(concat!(
        r"\b(?:time::(?:sleep|sleep_until|timeout|timeout_at)|thread::sleep)\b",
        r"|\.set_delay\(",
        r"|^\s*use\s+(?:tokio::time|std::thread)\b.*\b(?:sleep|sleep_until|timeout|timeout_at)\b",
        r"|(?:^|[^:.\w])(?:sleep|sleep_until|timeout|timeout_at)\(",
    ))
    .unwrap()
}

/// Each file's wall-clock waits in test code, as `(line, code)`.
fn wall_clock_waits(sources: &[SourceFile]) -> BTreeMap<String, Vec<(usize, String)>> {
    let pattern = wall_clock_wait();
    let mut found: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    for file in sources {
        if WAIT_HELPERS.contains(&file.path.as_str()) {
            continue;
        }
        for (number, code) in file.code_lines(true) {
            if code.trim_start().starts_with("fn ") || code.contains(" fn ") {
                // Declaring a helper named `timeout` isn't waiting.
                continue;
            }
            for _ in pattern.find_iter(code) {
                found
                    .entry(file.path.clone())
                    .or_default()
                    .push((number, code.trim().to_string()));
            }
        }
    }
    found
}

/// Files that may grant execute permission in test code: the helper that does
/// it safely, and this file, whose rule tests hold sample lines.
const EXEC_GRANT_HELPERS: &[&str] = &["src/testing/script.rs", "src/testing/source_scan.rs"];

/// Test code that sets an execute-bit mode on something that is not a script
/// the test then runs, as `(file, count, why)`. A directory needs its execute
/// bit to be entered, so making one read-only and restoring it is not writing
/// a program.
const EXEC_MODE_ALLOWED: &[(&str, usize, &str)] = &[
    (
        "src/update.rs",
        2,
        "makes a directory read-only (0o555) and restores it (0o755)",
    ),
    (
        "src/checkpoints/engine.rs",
        2,
        "makes an object directory unwritable (0o500) and restores it (0o700)",
    ),
    (
        "src/gateway/web/agent_keys.rs",
        2,
        "makes an object directory unwritable (0o500) and restores it (0o700) in a drop guard",
    ),
];

/// A test making a file executable on its own: a `chmod`, or a mode literal
/// with an execute bit passed to `from_mode`, `set_mode` or `.mode(`. The file
/// is then written in this process, and executing it races with any other
/// test thread that forks while the write descriptor is open.
fn exec_grant() -> Regex {
    Regex::new(r#""chmod"|\b(?:from_mode|set_mode|mode)\(\s*0o([0-7_]+)"#).unwrap()
}

/// How many execute-granting calls `code` holds.
fn exec_grants_on(pattern: &Regex, code: &str) -> usize {
    pattern
        .captures_iter(code)
        .filter(|call| {
            call.get(1).is_none_or(|digits| {
                let digits: String = digits.as_str().chars().filter(|c| *c != '_').collect();
                u32::from_str_radix(&digits, 8).is_ok_and(|mode| mode & 0o111 != 0)
            })
        })
        .count()
}

/// Each file's execute grants in test code, as `(line, code)`.
fn exec_grants(sources: &[SourceFile]) -> BTreeMap<String, Vec<(usize, String)>> {
    let pattern = exec_grant();
    let mut found: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    for file in sources {
        if EXEC_GRANT_HELPERS.contains(&file.path.as_str()) {
            continue;
        }
        for (number, code) in file.code_lines(true) {
            for _ in 0..exec_grants_on(&pattern, code) {
                found
                    .entry(file.path.clone())
                    .or_default()
                    .push((number, code.trim().to_string()));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source<'a>(sources: &'a [SourceFile], path: &str) -> &'a SourceFile {
        sources
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} is not in the crate"))
    }

    /// Whether the one line of `path` containing `needle` is test code.
    fn is_test_line(sources: &[SourceFile], path: &str, needle: &str) -> bool {
        let file = source(sources, path);
        let matches: Vec<bool> = file
            .lines
            .iter()
            .zip(&file.test)
            .filter(|(line, _)| line.contains(needle))
            .map(|(_, test)| *test)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "the anchor {needle:?} must match exactly one line of {path}"
        );
        matches.first().copied().unwrap()
    }

    #[test]
    fn the_classifier_tells_test_code_from_production_code() {
        let sources = crate_sources();
        let production = [
            (
                "src/agent/turn.rs",
                "use tokio_util::sync::CancellationToken;",
            ),
            (
                "src/agent/turn.rs",
                "tokio::time::sleep(std::time::Duration::from_millis(500)).await;",
            ),
            (
                "src/hub/host.rs",
                "pub fn publish(&self, event: HubEvent) {",
            ),
        ];
        for (path, needle) in production {
            assert!(
                !is_test_line(&sources, path, needle),
                "{path}: {needle:?} is production code"
            );
        }
        let test = [
            ("src/agent/turn.rs", "use tokio::sync::mpsc;"),
            (
                "src/hub/host.rs",
                "pub(crate) fn reloads_finished(&self, name: &str) -> Option<u64> {",
            ),
            (
                "src/hub/host.rs",
                ".map(|running| *running.control.reload_done.borrow())",
            ),
            // Below fake-tool scripts whose raw strings hold a column-0 `}`.
            (
                "src/interfaces/teams/setup_job.rs",
                "async fn test_job_failure_mid_provision_records_created_and_retry() {",
            ),
        ];
        for (path, needle) in test {
            assert!(
                is_test_line(&sources, path, needle),
                "{path}: {needle:?} is test code"
            );
        }
        for path in [
            "src/hub/host/tests.rs",
            "src/hub/host/tests/agent_watch.rs",
            "src/hub/test_support.rs",
            "tests/it/smoke.rs",
        ] {
            assert!(
                source(&sources, path)
                    .test
                    .iter()
                    .all(|line_is_test| *line_is_test),
                "{path} is test code throughout"
            );
        }
        assert!(
            is_test_gate("#[cfg(test)]"),
            "the test gate marks test code"
        );
        assert!(
            is_test_gate(r#"    #[cfg(any(test, feature = "test-support"))]"#),
            "the test-support gate marks test code, since its helpers compile into other crates' test builds"
        );
        assert!(
            !is_test_gate(r#"#[cfg(feature = "test-support")]"#),
            "a bare test-support feature gate is production code: it compiles into release builds"
        );
        assert!(
            !is_test_gate("#[cfg(not(test))]"),
            "a negated test gate is production code"
        );
    }

    /// An agent's tasks must carry its `agent` log field, and `tokio::spawn`
    /// starts a task with no tracing span. Production code spawns through
    /// `crate::util::spawn_in_span` and its siblings; this fails when a bare
    /// spawn slips back in.
    #[test]
    fn no_bare_spawns_outside_the_span_preserving_helpers() {
        let bare = [
            "tokio::spawn(",
            "tokio::task::spawn(",
            "tokio::task::spawn_blocking(",
            "std::thread::spawn(",
        ];
        let mut offenders = Vec::new();
        for file in crate_sources() {
            if file.path == "src/util/spawn.rs" {
                continue;
            }
            for (number, code) in file.code_lines(false) {
                if bare.iter().any(|call| code.contains(call)) {
                    offenders.push(format!("{}:{number}", file.path));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use crate::util::spawn_in_span / spawn_blocking_in_span so spawned tasks keep the agent span: {offenders:?}"
        );
    }

    /// Test code waits on the event it checks, through `crate::testing`. The
    /// waits that predate the rule are counted in `wall_clock_backlog`, and the
    /// counts may only go down.
    #[test]
    fn test_code_waits_on_events_not_elapsed_time() {
        let found = wall_clock_waits(&crate_sources());
        let listed: BTreeMap<&str, usize> = BACKLOG
            .iter()
            .copied()
            .chain(ALLOWED.iter().map(|(path, count, _)| (*path, *count)))
            .collect();
        let mut problems = String::new();
        for (path, waits) in &found {
            let count = waits.len();
            match listed.get(path.as_str()) {
                None => {
                    for (number, code) in waits {
                        writeln!(problems, "{path}:{number}: {code}").unwrap();
                    }
                }
                Some(&allowed) if count > allowed => {
                    writeln!(
                        problems,
                        "{path} has {count} wall-clock waits where {allowed} are listed; the new one should wait on an event instead:"
                    )
                    .unwrap();
                    for (number, code) in waits {
                        writeln!(problems, "  {path}:{number}: {code}").unwrap();
                    }
                }
                Some(&allowed) if count < allowed => {
                    writeln!(
                        problems,
                        "{path} is down to {count} wall-clock waits: lower its entry in src/testing/wall_clock_backlog.rs from {allowed} to {count}"
                    )
                    .unwrap();
                }
                Some(_) => {}
            }
        }
        for (path, _) in listed
            .iter()
            .filter(|(path, _)| !found.contains_key(**path))
        {
            writeln!(
                problems,
                "{path} has no wall-clock waits left (or is gone): remove its entry from src/testing/wall_clock_backlog.rs"
            )
            .unwrap();
        }
        assert!(
            problems.is_empty(),
            "test code waits on elapsed time. Wait on the event instead, with crate::testing::wait (channels, watches, polled conditions under one hang guard), crate::testing::gate (hold a call open until the test releases it) or crate::testing::clock (a paused clock, for code with no real I/O); CONTRIBUTING.md, \"Waiting in tests\", says which fits where.\n{problems}"
        );
    }

    /// A test that writes a script and executes it fails with "Text file
    /// busy" when another test thread forks while the write descriptor is
    /// open. `crate::testing::script::write_executable` writes through a child
    /// shell instead, so test code makes nothing executable by itself.
    #[test]
    fn test_code_makes_scripts_executable_through_the_helper() {
        let found = exec_grants(&crate_sources());
        let mut problems = String::new();
        for (path, grants) in &found {
            let allowed = EXEC_MODE_ALLOWED
                .iter()
                .find(|(allowed_path, _, _)| allowed_path == path)
                .map_or(0, |(_, count, _)| *count);
            if grants.len() != allowed {
                for (number, code) in grants {
                    writeln!(problems, "{path}:{number}: {code}").unwrap();
                }
            }
        }
        for (path, count, _) in EXEC_MODE_ALLOWED {
            let found_count = found.get(*path).map_or(0, Vec::len);
            if found_count != *count {
                writeln!(
                    problems,
                    "{path} has {found_count} execute grants where {count} are listed in EXEC_MODE_ALLOWED in src/testing/source_scan.rs: correct its entry"
                )
                .unwrap();
            }
        }
        assert!(
            problems.is_empty(),
            "test code makes a file executable itself. Write the script with crate::testing::script::write_executable, which never holds a write descriptor that a forked child could inherit (\"Text file busy\"); a test that only needs some program to run links to a system binary (/bin/sh, /bin/echo). A directory restored to a mode with its execute bit goes in EXEC_MODE_ALLOWED in src/testing/source_scan.rs, with why. CONTRIBUTING.md, \"Waiting in tests\", has the details.\n{problems}"
        );
    }

    #[test]
    fn the_executable_rule_matches_execute_grants_and_nothing_else() {
        let pattern = exec_grant();
        for grant in [
            "std::fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();",
            "perms.set_mode(0o700);",
            "options.mode(0o755).open(&path)",
            "Command::new(\"chmod\").arg(\"+x\")",
            "Permissions::from_mode(0o1_00)",
        ] {
            assert_eq!(
                exec_grants_on(&pattern, grant),
                1,
                "{grant:?} grants execute"
            );
        }
        for other in [
            "std::fs::Permissions::from_mode(0o600)",
            "Permissions::from_mode(0o644)",
            "perms.set_mode(0o400);",
            "options.mode(0o600);",
            "assert_eq!(mode & 0o777, 0o600);",
            "metadata.permissions().mode() & 0o111 != 0",
            "entry.mode().is_tree()",
            "Permissions::from_mode(mode)",
        ] {
            assert_eq!(
                exec_grants_on(&pattern, other),
                0,
                "{other:?} grants nothing"
            );
        }
    }

    #[test]
    fn lines_inside_strings_and_block_comments_are_marked() {
        let lines: Vec<String> = [
            r##"let script = r#"#!/bin/sh"##,
            "}",
            r##""#;"##,
            r#"let quote = '"';"#,
            "let s = \"two",
            "lines\";",
            "/* outer /* inner */",
            "still */",
            "fn after<'a>() {}",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            starts_inside_literal(&lines),
            [false, true, true, false, false, true, false, true, false]
        );
    }

    #[test]
    fn the_wall_clock_rule_matches_waits_and_nothing_else() {
        let pattern = wall_clock_wait();
        for wait in [
            "tokio::time::sleep(Duration::from_millis(5)).await;",
            "time::sleep_until(deadline).await;",
            "tokio::time::timeout(WAIT, rx.recv()).await",
            "std::thread::sleep(Duration::from_millis(5));",
            "use tokio::time::{Duration, timeout};",
            "sleep(Duration::from_millis(5)).await;",
            "ResponseTemplate::new(200).set_delay(delay)",
        ] {
            assert!(pattern.is_match(wait), "{wait:?} is a wall-clock wait");
        }
        for not_wait in [
            "tokio::time::advance(Duration::from_secs(1)).await;",
            "let deadline = tokio::time::Instant::now() + WAIT;",
            "reqwest::Client::builder().timeout(Duration::from_secs(5))",
            "self.runtime::sleep_count",
            "const STOP_TIMEOUT: Duration = Duration::from_secs(1);",
        ] {
            assert!(
                !pattern.is_match(not_wait),
                "{not_wait:?} is not a wall-clock wait"
            );
        }
    }

    /// The scan covers the root and every workspace member, and nothing else:
    /// a member that is not scanned hides its waits from every rule here. The
    /// expectation lists `crates/` from disk rather than reading `members`,
    /// so a member missing from `members` fails here.
    #[test]
    fn the_scanned_roots_are_the_root_and_every_workspace_member() {
        let root = crate_root();
        let mut crate_dirs = vec![String::new()];
        let crates_dir = root.join("crates");
        if crates_dir.is_dir() {
            for entry in std::fs::read_dir(&crates_dir).expect("list crates") {
                let path = entry.expect("list crates").path();
                if path.join("Cargo.toml").is_file() {
                    crate_dirs.push(relative(&root, &path));
                }
            }
        }
        let mut expected: Vec<String> = crate_dirs
            .iter()
            .flat_map(|dir| {
                ["src", "tests"].into_iter().map(move |leaf| {
                    if dir.is_empty() {
                        leaf.to_string()
                    } else {
                        format!("{dir}/{leaf}")
                    }
                })
            })
            .filter(|dir| root.join(dir).is_dir())
            .collect();
        expected.sort();
        let mut actual = scan_roots().expect("scan roots");
        actual.sort();
        assert_eq!(
            actual, expected,
            "the scanned roots are src and tests of the root and of each [workspace] member"
        );
    }

    /// A hard-coded path names a file that exists, so a rename cannot leave an
    /// exemption or a backlog entry silently pointing at nothing.
    #[test]
    fn the_hard_coded_paths_name_files_that_exist() {
        let root = crate_root();
        let source = std::fs::read_to_string(root.join("src/testing/source_scan.rs"))
            .expect("read source_scan.rs");
        let path_literal = Regex::new(r#""((?:src|tests|crates)/[^"\s]*\.rs)""#).unwrap();
        let mut named: Vec<String> = path_literal
            .captures_iter(&source)
            .map(|caps| caps[1].to_string())
            .collect();
        named.extend(BACKLOG.iter().map(|(path, _)| (*path).to_string()));
        named.extend(ALLOWED.iter().map(|(path, _, _)| (*path).to_string()));
        let missing: Vec<&str> = named
            .iter()
            .map(String::as_str)
            .filter(|path| !root.join(path).is_file())
            .collect();
        assert!(
            missing.is_empty(),
            "these paths name no file (a rename left them behind?): {missing:?}"
        );
    }
}
