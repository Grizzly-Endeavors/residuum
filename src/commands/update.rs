use residuum::util::FatalError;

#[derive(clap::Args)]
pub(super) struct UpdateArgs {
    #[arg(long)]
    pub check: bool,
    #[arg(long, short)]
    pub yes: bool,
}

#[tracing::instrument(skip_all)]
pub(super) async fn run_update_command(args: &UpdateArgs) -> Result<(), FatalError> {
    use residuum::hub::update;

    let current = residuum::BUILD_INFO.version;
    println!("residuum: checking for updates...");

    let latest = update::fetch_latest_version().await?;

    if update::is_up_to_date(current, &latest) {
        println!("residuum: already up to date ({current})");
        return Ok(());
    }

    println!("residuum: current version: {current}");
    println!("residuum: latest version:  {latest}");

    if args.check {
        return Ok(());
    }

    println!("residuum: downloading and installing {latest}...");

    let installed = update::download_and_install(current, &latest).await?;
    println!("residuum: updated to {latest}");
    if installed.verification == update::ReleaseVerification::Unverified {
        println!(
            "residuum: this update couldn't be verified — the release had no checksum to check the download against"
        );
    }

    if let Ok(pid_path) = residuum::daemon::pid_file_path()
        && let Ok(pid) = residuum::daemon::read_pid_file(&pid_path)
        && residuum::daemon::is_process_running(pid)
    {
        if args.yes {
            let gateway_addr = super::resolve_gateway_addr(&residuum::config::residuum_root()?);
            let url = format!("http://{gateway_addr}/api/hub/update/restart");
            let response = match super::hub_client::local_hub_client().build() {
                Ok(client) => client.post(&url).send().await,
                Err(e) => Err(e),
            };
            match response {
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
