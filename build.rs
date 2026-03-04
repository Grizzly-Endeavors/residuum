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
}
