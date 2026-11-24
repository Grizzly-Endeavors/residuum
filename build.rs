fn main() {
    let dist = std::path::Path::new("web/dist/index.html");
    assert!(
        dist.exists(),
        "web/dist/ not found — run `npm run build` in the web/ directory first"
    );
    println!("cargo:rerun-if-changed=web/dist/");

    let version = from_build_env("RESIDUUM_VERSION")
        .or_else(|| git_stdout(&["describe", "--tags", "--always"]))
        .unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=RESIDUUM_VERSION={version}");

    if let Some(commit) = from_build_env("RESIDUUM_GIT_COMMIT")
        .or_else(|| git_stdout(&["rev-parse", "--short=12", "HEAD"]))
    {
        println!("cargo:rustc-env=RESIDUUM_GIT_COMMIT={commit}");
    }

    watch_git_state();
}

fn from_build_env(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_stdout(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn watch_git_state() {
    for git_path in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git_stdout(&["rev-parse", "--git-path", git_path])
            && std::path::Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
