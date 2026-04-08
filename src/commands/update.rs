//! Update subcommand: check for and install updates from GitHub Releases.

use residuum::util::FatalError;

#[derive(clap::Args)]
pub(super) struct UpdateArgs {
    /// Only check for updates, don't install
    #[arg(long)]
    pub check: bool,
    /// Automatically restart the gateway after updating
    #[arg(long, short)]
    pub yes: bool,
}

/// Check for and install updates from GitHub Releases.
///
/// Fetches the latest release tag, compares it against the build-time
/// version, and optionally downloads the install script to replace the
/// current binary. With `-y`/`--yes`, automatically triggers a daemon
/// restart after a successful update.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the GitHub API request fails or
/// the install script cannot be executed.
#[tracing::instrument(skip_all)]
pub(super) async fn run_update_command(args: &UpdateArgs) -> Result<(), FatalError> {
    use residuum::update::{self, CURRENT_VERSION};

    println!("residuum: checking for updates...");

    let latest = update::fetch_latest_version().await?;

    if update::is_up_to_date(CURRENT_VERSION, &latest) {
        println!("residuum: already up to date ({CURRENT_VERSION})");
        return Ok(());
    }

    println!("residuum: current version: {CURRENT_VERSION}");
    println!("residuum: latest version:  {latest}");

    if args.check {
        return Ok(());
    }

    println!("residuum: downloading and installing {latest}...");

    update::download_and_install(&latest).await?;
    println!("residuum: updated to {latest}");

    // Check if gateway is running and try to restart it
    if let Ok(pid_path) = residuum::daemon::pid_file_path()
        && let Ok(pid) = residuum::daemon::read_pid_file(&pid_path)
        && residuum::daemon::is_process_running(pid)
    {
        if args.yes {
            // Try to trigger seamless restart via the API
            let config_dir = residuum::agent_registry::paths::resolve_config_dir(None)?;
            let gateway_addr = super::resolve_gateway_addr(&config_dir);
            let url = format!("http://{gateway_addr}/api/update/restart");
            match reqwest::Client::new().post(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    println!("residuum: restart signal sent to gateway (pid {pid})");
                }
                Ok(resp) => {
                    println!(
                        "residuum: failed to signal gateway restart (status {}) — restart it manually",
                        resp.status()
                    );
                }
                Err(e) => {
                    println!(
                        "residuum: failed to signal gateway restart ({e}) — restart it manually"
                    );
                }
            }
        } else {
            println!(
                "residuum: gateway is still running (pid {pid}) — restart it to use the new version"
            );
        }
    }

    Ok(())
}
