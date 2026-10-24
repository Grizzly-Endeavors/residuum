fn main() {
    let dist = std::path::Path::new("web/dist/index.html");
    assert!(
        dist.exists(),
        "web/dist/ not found — run `npm run build` in the web/ directory first"
    );
    println!("cargo:rerun-if-changed=web/dist/");

    // The version for the update command and bug reports: `RESIDUUM_VERSION` from
    // the build's environment when set (an image built from `docker/Dockerfile`,
    // whose context leaves out `.git`, gets it as a build arg), else from git.
    // Tagged commit: v2026.03.02. Between tags: v2026.03.02-5-gabcdef1. Neither: dev.
    let version = from_build_env("RESIDUUM_VERSION")
        .or_else(|| git_stdout(&["describe", "--tags", "--always"]))
        .unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=RESIDUUM_VERSION={version}");

    // Short git commit hash for bug-report client context, from the environment
    // or git in the same way. Distinct from RESIDUUM_VERSION so reports can carry
    // both a tag and a SHA. Unset (option_env! → None) when neither has one, so
    // the field serializes as null.
    if let Some(commit) = from_build_env("RESIDUUM_GIT_COMMIT")
        .or_else(|| git_stdout(&["rev-parse", "--short=12", "HEAD"]))
    {
        println!("cargo:rustc-env=RESIDUUM_GIT_COMMIT={commit}");
    }

    watch_git_state();
}

/// The trimmed value of `name` in the build script's environment, or `None` when
/// it is unset or blank. Cargo reruns the script when it changes.
fn from_build_env(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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

/// Reruns the build script when this worktree's checked-out commit changes, so the
/// baked-in version and commit hash track it. Two per-worktree files are watched:
/// `HEAD`, which a checkout or switch rewrites, and the `HEAD` reflog (`logs/HEAD`),
/// which is appended to on every move of `HEAD` in this worktree: a commit on the
/// checked-out branch, a reset, a rebase, a commit on a detached `HEAD`. The branch
/// ref and `packed-refs` are not watched: `packed-refs` is shared by every worktree
/// and rewritten when a branch is deleted or refs are packed, which would rebuild
/// the crate everywhere without a commit changing, and the branch ref is a loose
/// file only until refs are packed. A `HEAD` move that keeps the same commit
/// (`reset --hard HEAD`, `stash`) reruns the script once in that worktree. Without
/// a reflog (`core.logAllRefUpdates=false`) only `HEAD` is watched, which still
/// catches checkouts and detached commits. Paths come from git itself since a
/// worktree's `.git` is a file, not a directory. Only existing paths are emitted:
/// cargo treats a missing watched path as always changed.
fn watch_git_state() {
    for git_path in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git_stdout(&["rev-parse", "--git-path", git_path])
            && std::path::Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
