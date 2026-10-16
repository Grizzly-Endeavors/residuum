//! Rules checked by reading the workspace's own source: the root crate's and
//! every workspace member's `src` and `tests`.
//!
//! The rules need to know which lines are test code, and a file is not split
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
//!
//! The same classification under `#[cfg(test)]` alone gives the code only the
//! crate's own test build compiles, without the test-support helpers other
//! crates' tests use. `unused_pub` needs that split: a member crate's `pub`
//! item that only its own tests use is dead to every other crate.

mod unused_pub;

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

/// Whether `line` is `#[cfg(test)]`, which compiles the item after it into
/// its own crate's test build alone.
fn is_own_test_gate(line: &str) -> bool {
    line.trim() == "#[cfg(test)]"
}

/// A source file, its lines, and which of them are test code.
struct SourceFile {
    /// Relative to the repository root, with `/` separators.
    path: String,
    lines: Vec<String>,
    test: Vec<bool>,
    /// The lines only the crate's own test build compiles: `test` without the
    /// test-support code, which other crates' test builds compile too.
    own_test: Vec<bool>,
}

impl SourceFile {
    /// Classifies `lines`. `test_file` and `own_test_file` say whether module
    /// declarations make the whole file test code: under either gate, and
    /// under `#[cfg(test)]` alone.
    fn new(path: String, lines: Vec<String>, test_file: bool, own_test_file: bool) -> Self {
        let in_literal = starts_inside_literal(&lines);
        let test = gated_lines(&lines, &in_literal, test_file, is_test_gate);
        let own_test = gated_lines(&lines, &in_literal, own_test_file, is_own_test_gate);
        Self {
            path,
            lines,
            test,
            own_test,
        }
    }

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

/// For each line, whether `gate` covers it: every line when `whole_file` is
/// set, else the lines of each item a `gate` line precedes.
fn gated_lines(
    lines: &[String],
    in_literal: &[bool],
    whole_file: bool,
    gate: fn(&str) -> bool,
) -> Vec<bool> {
    let mut flags = vec![whole_file; lines.len()];
    if whole_file {
        return flags;
    }
    let mut idx = 0;
    while idx < lines.len() {
        if lines.get(idx).is_some_and(|line| gate(line)) {
            let end = item_end(lines, in_literal, item_start(lines, idx));
            for flag in flags.iter_mut().take(end + 1).skip(idx) {
                *flag = true;
            }
            idx = end + 1;
        } else {
            idx += 1;
        }
    }
    flags
}

/// The files that are test code throughout under `gate`: each file under one
/// of `test_dirs`, each file a `mod x;` after a `gate` line declares, and
/// everything a test file declares, until nothing new turns up.
fn test_files(
    contents: &BTreeMap<PathBuf, Vec<String>>,
    test_dirs: &[PathBuf],
    gate: fn(&str) -> bool,
) -> BTreeSet<PathBuf> {
    let mod_decl = Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z0-9_]+)\s*;").unwrap();
    let mut found: BTreeSet<PathBuf> = contents
        .keys()
        .filter(|path| test_dirs.iter().any(|dir| path.starts_with(dir)))
        .cloned()
        .collect();
    let mut pending: Vec<PathBuf> = Vec::new();
    for (path, lines) in contents {
        for (idx, line) in lines.iter().enumerate() {
            if !gate(line) {
                continue;
            }
            let start = item_start(lines, idx);
            if let Some(name) = lines.get(start).and_then(|item| mod_decl.captures(item))
                && let Some(file) = module_file(path, &name[1])
                && found.insert(file.clone())
            {
                pending.push(file);
            }
        }
    }
    pending.extend(found.iter().cloned());
    while let Some(path) = pending.pop() {
        let Some(lines) = contents.get(&path) else {
            continue;
        };
        for line in lines {
            if let Some(name) = mod_decl.captures(line)
                && let Some(file) = module_file(&path, &name[1])
                && found.insert(file.clone())
            {
                pending.push(file);
            }
        }
    }
    found
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
    let test_code = test_files(&contents, &test_dirs, is_test_gate);
    let own_test_code = test_files(&contents, &test_dirs, is_own_test_gate);
    contents
        .into_iter()
        .map(|(path, lines)| {
            let test_file = test_code.contains(&path);
            let own_test_file = own_test_code.contains(&path);
            SourceFile::new(relative(&root, &path), lines, test_file, own_test_file)
        })
        .collect()
}

/// Files whose test code may wait on the clock: the waits that everything
/// else goes through.
const WAIT_HELPERS: &[&str] = &[
    "crates/core/src/testing/wait.rs",
    "crates/core/src/testing/clock.rs",
    "src/testing/source_scan.rs",
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
const EXEC_GRANT_HELPERS: &[&str] = &[
    "crates/core/src/testing/script.rs",
    "src/testing/source_scan.rs",
];

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

/// Where the integration tests live, and the file that declares them. Each
/// top-level module of `dir` (a file, or a directory with a `mod.rs`) carries
/// one `//! covers:` line naming the packages it tests, except the `exempt`
/// modules. A directory marked `optional` may be absent; any other must exist.
struct ModuleDir {
    dir: &'static str,
    container: &'static str,
    exempt: &'static [&'static str],
    optional: bool,
}

const MODULE_DIRS: [ModuleDir; 2] = [
    ModuleDir {
        dir: "tests/it",
        container: "main.rs",
        exempt: &["support"],
        optional: false,
    },
    ModuleDir {
        dir: "src/integration",
        container: "mod.rs",
        exempt: &[],
        optional: true,
    },
];

const PENDING_CRATES: &str = "scripts/pending-crates.txt";

/// The `name` of each member package under `root`, read from its manifest.
fn member_packages(root: &Path) -> Result<BTreeSet<String>, String> {
    let manifest_path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|err| format!("failed to read {}: {err}", manifest_path.display()))?;
    let manifest: toml::Table = toml::from_str(&text)
        .map_err(|err| format!("failed to parse {}: {err}", manifest_path.display()))?;
    let members = workspace_members(&manifest)?;
    let mut packages = BTreeSet::new();
    for dir in member_dirs(root, &members)? {
        let member_path = root.join(&dir).join("Cargo.toml");
        let member_text = std::fs::read_to_string(&member_path)
            .map_err(|err| format!("failed to read {}: {err}", member_path.display()))?;
        let member: toml::Table = toml::from_str(&member_text)
            .map_err(|err| format!("failed to parse {}: {err}", member_path.display()))?;
        let name = member
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("{} has no [package] name", member_path.display()))?;
        packages.insert(name.to_string());
    }
    Ok(packages)
}

/// The `<path> <package>` lines of `scripts/pending-crates.txt`. Each path must
/// exist: a directory when it ends in `/`, a file otherwise.
fn pending_crates(root: &Path, problems: &mut Vec<String>) -> Vec<(String, String)> {
    let path = root.join(PENDING_CRATES);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            problems.push(format!("failed to read {PENDING_CRATES}: {err}"));
            return Vec::new();
        }
    };
    let mut entries = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let number = idx + 1;
        let fields: Vec<&str> = line.split_whitespace().collect();
        let &[rel, package] = fields.as_slice() else {
            problems.push(format!(
                "{PENDING_CRATES}:{number} must be `<path> <package>`"
            ));
            continue;
        };
        let target = root.join(rel.trim_end_matches('/'));
        let exists = if rel.ends_with('/') {
            target.is_dir()
        } else {
            target.is_file()
        };
        if !exists {
            problems.push(format!(
                "{PENDING_CRATES}:{number} names {rel}, which is not an existing {}",
                if rel.ends_with('/') {
                    "directory"
                } else {
                    "file"
                }
            ));
        }
        entries.push((rel.to_string(), package.to_string()));
    }
    entries
}

/// The leading `//!` doc lines of a file that concern its covers line: the names
/// of each canonical `//! covers: <package>, …` line, and the text of each doc
/// line that tries to be one and is not in that form.
struct CoversScan {
    lines: Vec<Vec<String>>,
    malformed: Vec<String>,
}

/// The names of a covers line's list, when the list is `<package>, <package>, …`
/// with single spaces and no other separator.
fn canonical_names(list: &str) -> Option<Vec<String>> {
    let names: Vec<String> = list.split(", ").map(str::to_string).collect();
    names
        .iter()
        .all(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .then_some(names)
}

/// The covers lines of the leading doc comment of `file`.
fn covers_lines(file: &Path) -> Result<CoversScan, String> {
    let text = std::fs::read_to_string(file)
        .map_err(|err| format!("failed to read {}: {err}", file.display()))?;
    let mut scan = CoversScan {
        lines: Vec::new(),
        malformed: Vec::new(),
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some(doc) = line.trim_start().strip_prefix("//!") else {
            break;
        };
        if let Some(list) = line.strip_prefix("//! covers: ") {
            match canonical_names(list) {
                Some(names) => scan.lines.push(names),
                None => scan.malformed.push(line.to_string()),
            }
        } else if doc
            .trim()
            .strip_prefix("covers")
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
        {
            scan.malformed.push(line.to_string());
        }
    }
    Ok(scan)
}

/// Every `.rs` file under the module directory `dir`, at any depth, sorted.
fn files_below_module(dir: &Path, problems: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(err) => {
                problems.push(format!("failed to list {}: {err}", current.display()));
                continue;
            }
        };
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(err) => {
                    problems.push(format!("failed to list {}: {err}", current.display()));
                    continue;
                }
            };
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Every top-level module of `dir`, as its name and the file that holds it,
/// sorted by name. The `container` file is not a module.
fn top_level_modules(
    dir: &Path,
    container: &str,
    problems: &mut Vec<String>,
) -> Vec<(String, PathBuf)> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            problems.push(format!("failed to list {}: {err}", dir.display()));
            return Vec::new();
        }
    };
    let mut modules = Vec::new();
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(err) => {
                problems.push(format!("failed to list {}: {err}", dir.display()));
                continue;
            }
        };
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name == container {
            continue;
        }
        if path.is_dir() {
            let module_file = path.join("mod.rs");
            if module_file.is_file() {
                modules.push((file_name.to_string(), module_file));
            } else {
                problems.push(format!("{} has no mod.rs", path.display()));
            }
        } else if let Some(stem) = file_name.strip_suffix(".rs") {
            modules.push((stem.to_string(), path.clone()));
        }
    }
    modules.sort();
    modules
}

/// Problems with the covers lines, the `mod` lines that declare the modules
/// under `MODULE_DIRS`, and `scripts/pending-crates.txt`. An empty result is a
/// pass. A missing directory fails unless it is optional.
fn covers_problems(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let mut valid_packages = BTreeSet::from(["residuum".to_string()]);
    match member_packages(root) {
        Ok(packages) => valid_packages.extend(packages),
        Err(err) => problems.push(err),
    }
    for (_, package) in pending_crates(root, &mut problems) {
        valid_packages.insert(package);
    }
    let mod_decl = Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z0-9_]+)\s*;").unwrap();
    for module_dir in &MODULE_DIRS {
        let dir = root.join(module_dir.dir);
        if !dir.is_dir() {
            if !module_dir.optional {
                problems.push(format!("{} does not exist", module_dir.dir));
            }
            continue;
        }
        let container_path = dir.join(module_dir.container);
        let Ok(container_text) = std::fs::read_to_string(&container_path) else {
            problems.push(format!(
                "{} has no {}, which declares its modules",
                module_dir.dir, module_dir.container
            ));
            continue;
        };
        let declared: BTreeSet<String> = container_text
            .lines()
            .filter_map(|line| mod_decl.captures(line).map(|caps| caps[1].to_string()))
            .collect();
        let modules = top_level_modules(&dir, module_dir.container, &mut problems);
        let module_names: BTreeSet<String> = modules.iter().map(|(name, _)| name.clone()).collect();
        for name in &module_names {
            if !declared.contains(name) {
                problems.push(format!(
                    "{}/{} has no `mod {name};` line",
                    module_dir.dir, module_dir.container
                ));
            }
        }
        for name in declared.difference(&module_names) {
            problems.push(format!(
                "{}/{} declares `mod {name};` with no file",
                module_dir.dir, module_dir.container
            ));
        }
        for (name, file) in &modules {
            let rel = format!("{}/{name}", module_dir.dir);
            let module_dir_path = dir.join(name);
            if module_dir_path.is_dir() {
                // Only the module's mod.rs carries its covers line; every other
                // file below the directory belongs to the module.
                for inner in files_below_module(&module_dir_path, &mut problems) {
                    if inner == *file {
                        continue;
                    }
                    let inner_rel = inner.strip_prefix(root).unwrap_or(&inner).display();
                    match covers_lines(&inner) {
                        Ok(scan) if scan.lines.is_empty() && scan.malformed.is_empty() => {}
                        Ok(_) => problems.push(format!(
                            "{inner_rel} has a `//! covers:` line; only the mod.rs of its module carries one"
                        )),
                        Err(err) => problems.push(err),
                    }
                }
            }
            if module_dir.exempt.contains(&name.as_str()) {
                continue;
            }
            match covers_lines(file) {
                Ok(scan) => check_covers_lines(&rel, &scan, &valid_packages, &mut problems),
                Err(err) => problems.push(err),
            }
        }
    }
    problems
}

/// One module's covers lines: exactly one, canonical, naming packages that are
/// valid, with no name repeated.
fn check_covers_lines(
    rel: &str,
    scan: &CoversScan,
    valid_packages: &BTreeSet<String>,
    problems: &mut Vec<String>,
) {
    for line in &scan.malformed {
        problems.push(format!(
            "{rel} has the malformed covers line `{line}`; write it as `//! covers: <package>, …`"
        ));
    }
    let [names] = scan.lines.as_slice() else {
        problems.push(format!(
            "{rel} has {} `//! covers:` lines; it needs exactly one",
            scan.lines.len()
        ));
        return;
    };
    let mut seen = BTreeSet::new();
    for name in names {
        if !valid_packages.contains(name) {
            problems.push(format!(
                "{rel} names {name}, which is neither residuum, a member package, nor a package in {PENDING_CRATES}"
            ));
        } else if !seen.insert(name) {
            problems.push(format!("{rel} names {name} more than once"));
        }
    }
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

    /// Whether the one line of `path` containing `needle` is test code, and
    /// whether it is code only its own crate's tests compile.
    fn test_and_own_test(sources: &[SourceFile], path: &str, needle: &str) -> (bool, bool) {
        let file = source(sources, path);
        let matches: Vec<(bool, bool)> = file
            .lines
            .iter()
            .zip(file.test.iter().zip(&file.own_test))
            .filter(|(line, _)| line.contains(needle))
            .map(|(_, (test, own_test))| (*test, *own_test))
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "the anchor {needle:?} must match exactly one line of {path}"
        );
        matches.first().copied().unwrap()
    }

    #[test]
    fn the_classifier_tells_own_test_code_from_test_support_code() {
        let sources = crate_sources();
        let cases = [
            // A whole file behind the test-support gate.
            (
                "crates/core/src/testing/wait.rs",
                "pub fn drain<R: Recv>(rx: &mut R) -> Vec<R::Item> {",
                (true, false),
            ),
            // Its `#[cfg(test)] mod tests`.
            (
                "crates/core/src/testing/wait.rs",
                "async fn guarded_returns_what_the_future_does() {",
                (true, true),
            ),
            // An item behind the test-support gate in production code.
            (
                "crates/config/src/layout.rs",
                "pub fn with_tempdir_guard(mut self, dir: tempfile::TempDir) -> Self {",
                (true, false),
            ),
            (
                "crates/config/src/layout.rs",
                "pub fn new(root: impl Into<PathBuf>) -> Self {",
                (false, false),
            ),
            (
                "tests/it/smoke.rs",
                "fn binary_starts_without_panic() {",
                (true, true),
            ),
        ];
        for (path, needle, expected) in cases {
            assert_eq!(
                test_and_own_test(&sources, path, needle),
                expected,
                "{path}: {needle:?} is (test code, own-test code)"
            );
        }
        assert!(
            is_own_test_gate("    #[cfg(test)]"),
            "#[cfg(test)] marks own-test code"
        );
        assert!(
            !is_own_test_gate(r#"#[cfg(any(test, feature = "test-support"))]"#),
            "the test-support gate compiles into other crates' tests too"
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
            if file.path == "crates/core/src/util/spawn.rs" {
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
            "test code waits on elapsed time. Wait on the event instead, with residuum_core::testing::wait (channels, watches, polled conditions under one hang guard), residuum_core::testing::gate (hold a call open until the test releases it) or residuum_core::testing::clock (a paused clock, for code with no real I/O); CONTRIBUTING.md, \"Waiting in tests\", says which fits where.\n{problems}"
        );
    }

    /// A test that writes a script and executes it fails with "Text file
    /// busy" when another test thread forks while the write descriptor is
    /// open. `residuum_core::testing::script::write_executable` writes through a child
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
            "test code makes a file executable itself. Write the script with residuum_core::testing::script::write_executable, which never holds a write descriptor that a forked child could inherit (\"Text file busy\"); a test that only needs some program to run links to a system binary (/bin/sh, /bin/echo). A directory restored to a mode with its execute bit goes in EXEC_MODE_ALLOWED in src/testing/source_scan.rs, with why. CONTRIBUTING.md, \"Waiting in tests\", has the details.\n{problems}"
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

    /// Each `tests/it` and `src/integration` module carries one canonical covers
    /// line naming real packages, no file inside a module directory carries one,
    /// the `mod` lines match the files, and every path in
    /// `scripts/pending-crates.txt` exists.
    #[test]
    fn it_covers() {
        let problems = covers_problems(&crate_root());
        assert!(
            problems.is_empty(),
            "covers and pending-crates problems:\n{}",
            problems.join("\n")
        );
    }
}
