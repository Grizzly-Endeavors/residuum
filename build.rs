fn main() {
    let dist = std::path::Path::new("web/dist/index.html");
    assert!(
        dist.exists(),
        "web/dist/ not found — run `npm run build` in the web/ directory first"
    );
    println!("cargo:rerun-if-changed=web/dist/");

    // Capture git version for the update command.
    // Tagged commit: v2026.03.02. Between tags: v2026.03.02-5-gabcdef1. No git: dev.
    let version = std::process::Command::new("git")
        .args(["describe", "--tags", "--always"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "dev".to_string(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        );
    println!("cargo:rustc-env=RESIDUUM_VERSION={version}");

    // Capture short git commit hash for bug-report client context.
    // Distinct from RESIDUUM_VERSION so reports can carry both a tag and a SHA.
    // Unset (option_env! → None) when git is unavailable so the field serializes as null.
    if let Some(commit) = git_stdout(&["rev-parse", "--short=12", "HEAD"]) {
        println!("cargo:rustc-env=RESIDUUM_GIT_COMMIT={commit}");
    }

    watch_git_state();
}

/// Trimmed stdout of a successful git invocation, or `None` when git is
/// unavailable, fails, or this is not a git checkout.
fn git_stdout(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Reruns the build script when the checked-out commit changes, so the baked-in
/// version and commit hash track it. HEAD is watched because a checkout rewrites it,
/// the ref HEAD points at because a commit advances it, and `packed-refs` because
/// the ref may live there instead of as a loose file. Paths come from git itself
/// since a worktree's `.git` is a file, not a directory. Only existing paths are
/// emitted: cargo treats a missing watched path as always changed.
fn watch_git_state() {
    let mut paths = Vec::new();
    if let Some(head) = git_stdout(&["rev-parse", "--git-path", "HEAD"]) {
        paths.push(head);
    }
    if let Some(head_ref) = git_stdout(&["symbolic-ref", "-q", "HEAD"])
        && let Some(ref_path) = git_stdout(&["rev-parse", "--git-path", &head_ref])
    {
        paths.push(ref_path);
    }
    if let Some(common_dir) = git_stdout(&["rev-parse", "--git-common-dir"]) {
        paths.push(format!("{common_dir}/packed-refs"));
    }
    for path in paths {
        if std::path::Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
