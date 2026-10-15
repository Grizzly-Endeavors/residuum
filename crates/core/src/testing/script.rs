//! Writing a fake tool that a test then runs.
//!
//! Executing a file this process just wrote fails with "Text file busy" when
//! a test on another thread forks while the file is open for writing: the
//! forked child holds a copy of the write descriptor until it execs, and the
//! kernel refuses to run a file anything holds open for writing. The window
//! is short on an idle machine and wide under load. [`write_executable`]
//! never opens the file in this process, so no fork can carry its
//! descriptor. A test that only needs some program to run links to a system
//! binary (`/bin/sh`, `/bin/echo`) instead.

use std::path::Path;

/// Write an executable script at `path` with `content`, creating its parent
/// directory, ready to exec on return.
///
/// # Panics
/// Panics when the script can't be written, failing the test that asked for it.
#[expect(
    clippy::unwrap_used,
    reason = "test-support helper: a fake tool that can't be written fails the test that needs it"
)]
#[cfg(unix)]
pub fn write_executable(path: &Path, content: &str) {
    use std::io::Write as _;
    use std::process::Stdio;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    // A child shell writes the script, so the only write descriptor to it
    // lives in that child and is closed once it exits.
    let mut writer = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    writer
        .stdin
        .take()
        .unwrap()
        .write_all(content.as_bytes())
        .unwrap();
    let status = writer.wait().unwrap();
    assert!(
        status.success(),
        "writing fake tool {} failed",
        path.display()
    );
}

/// Write a script at `path` with `content`, creating its parent directory.
/// Windows does not hand a file handle to a child process unless the handle
/// was made inheritable, so writing it here carries no fork race.
///
/// # Panics
/// Panics when the script can't be written, failing the test that asked for it.
#[expect(
    clippy::unwrap_used,
    reason = "test-support helper: a fake tool that can't be written fails the test that needs it"
)]
#[cfg(not(unix))]
pub fn write_executable(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}
