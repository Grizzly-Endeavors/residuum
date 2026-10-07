//! `remote` subcommand: pair browsers for remote access through Residuum Cloud.
//!
//! Every subcommand is a client of the running hub's pairing routes. The first
//! device is paired from the machine Residuum runs on, never over the relay,
//! so a server used only remotely is paired over SSH with `residuum remote pair`.

use std::fmt::Write as _;

use reqwest::Method;

use residuum::pairing::qr;
use residuum::pairing::types::{DeviceListResponse, PairLinkResponse};
use residuum::remote_access::status::{RemoteAccessState, RemoteAccessStatus};
use residuum::util::FatalError;

use super::hub_client::HubClient;

/// Remote-access subcommands.
#[derive(clap::Subcommand)]
pub(super) enum RemoteCommand {
    /// Print a link and QR code that pairs a browser for remote access
    Pair,
    /// List the paired browsers and any waiting to be paired
    Devices,
    /// Stop trusting a paired browser
    Revoke {
        /// The device's id, from `residuum remote devices`
        id: String,
    },
    /// Show whether remote access is ready, its addresses, and the recovery code waiting to be saved
    Status,
    /// Tell Residuum the recovery code is saved, so it forgets it
    Saved,
    /// Take your address back after losing every instance's certificate account
    ResetPins {
        /// The recovery code from when remote access was first set up. Asked for when left out.
        #[arg(long)]
        recovery_code: Option<String>,
    },
}

/// Run the `remote` subcommand against the hub at `gateway_addr`.
pub(super) async fn run_remote_command(
    command: &RemoteCommand,
    gateway_addr: &str,
) -> Result<(), FatalError> {
    let client = HubClient::new(gateway_addr)?;
    match command {
        RemoteCommand::Pair => pair(&client).await,
        RemoteCommand::Devices => devices(&client).await,
        RemoteCommand::Revoke { id } => revoke(&client, id).await,
        RemoteCommand::Status => status(&client).await,
        RemoteCommand::Saved => saved(&client).await,
        RemoteCommand::ResetPins { recovery_code } => {
            reset_pins(&client, recovery_code.as_deref()).await
        }
    }
}

async fn pair(client: &HubClient) -> Result<(), FatalError> {
    let minted: PairLinkResponse = client
        .send(Method::POST, "/api/hub/remote-access/pair-link", None)
        .await?;
    // A terminal that can't draw the QR code still gets the link.
    let art = qr::terminal(&minted.link).ok();
    println!("{}", pairing_instructions(&minted, art.as_deref()));
    Ok(())
}

/// What `residuum remote pair` prints.
fn pairing_instructions(minted: &PairLinkResponse, qr_art: Option<&str>) -> String {
    let minutes = minted.expires_in_secs / 60;
    let mut out = String::new();
    out.push_str("Open this link in the browser you want to pair:\n\n");
    _ = writeln!(out, "  {}\n", minted.link);
    if let Some(art) = qr_art {
        out.push_str("Or scan it with a phone:\n\n");
        out.push_str(art);
        out.push('\n');
    }
    _ = writeln!(
        out,
        "The link works once and expires in {minutes} minutes. Run this command again for a new one."
    );
    if let Some(codes) = &minted.recovery_codes {
        out.push_str(
            "\nRecovery codes (each pairs one browser if you lose access to every paired one).\n\
             Save them now. They are shown only this once:\n\n",
        );
        for code in codes {
            _ = writeln!(out, "  {code}");
        }
    }
    out
}

async fn devices(client: &HubClient) -> Result<(), FatalError> {
    let listing: DeviceListResponse = client.send(Method::GET, "/api/hub/devices", None).await?;
    println!("{}", device_table(&listing));
    Ok(())
}

fn device_table(listing: &DeviceListResponse) -> String {
    let mut out = String::new();
    if listing.devices.is_empty() {
        out.push_str("No browsers are paired. Run `residuum remote pair` to pair one.\n");
    } else {
        out.push_str("Paired browsers:\n");
        for device in &listing.devices {
            _ = writeln!(
                out,
                "  {}  {}  paired {}, last seen {}",
                device.id,
                device.name,
                device.created_at.format("%Y-%m-%d"),
                device.last_seen.format("%Y-%m-%d %H:%M UTC"),
            );
        }
    }
    if !listing.pending.is_empty() {
        out.push_str("\nWaiting to be paired (approve from a paired browser or Settings):\n");
        for pending in &listing.pending {
            _ = writeln!(out, "  {}  {}", pending.code, pending.device_name);
        }
    }
    _ = write!(
        out,
        "\nUnused recovery codes: {}",
        listing.recovery_codes_remaining
    );
    out
}

async fn revoke(client: &HubClient, id: &str) -> Result<(), FatalError> {
    let path = format!("/api/hub/devices/{id}");
    client.send_no_content(Method::DELETE, &path).await?;
    println!("Revoked {id}. That browser is signed out of remote access.");
    Ok(())
}

async fn status(client: &HubClient) -> Result<(), FatalError> {
    let status: RemoteAccessStatus = client
        .send(Method::GET, "/api/hub/remote-access/status", None)
        .await?;
    println!("{}", status_report(&status));
    Ok(())
}

/// What `residuum remote status` prints.
fn status_report(status: &RemoteAccessStatus) -> String {
    let mut out = String::new();
    _ = writeln!(out, "Remote access: {}", state_summary(status.state));
    if let Some(detail) = &status.detail {
        _ = writeln!(out, "{detail}");
    }
    if let Some(hosts) = &status.hosts {
        _ = write!(
            out,
            "\nAddresses:\n  Residuum:   https://{}\n  Workbench:  https://{}\n  This instance (A2A, Teams): https://{}\n",
            hosts.ui, hosts.workbench, hosts.instance
        );
    }
    if let Some(certificate) = &status.certificate {
        _ = writeln!(
            out,
            "\nCertificate expires {}, renewal starts {}.",
            certificate.not_after, certificate.renews_at
        );
    }
    let unknown = status.unknown_pins();
    if !unknown.is_empty() {
        out.push_str(
            "\nWARNING: these certificate accounts can issue certificates for your addresses, and this instance never approved them:\n",
        );
        for pin in unknown {
            _ = writeln!(out, "  {} (instance {})", pin.account_uri, pin.slug);
        }
    }
    if let Some(code) = &status.recovery_code {
        _ = write!(
            out,
            "\nRecovery code: {code}\nThis is the only way to take your address back if every instance's certificate account is lost.\nSave it somewhere safe, then run `residuum remote saved` so Residuum stops keeping it.\n"
        );
    }
    out
}

fn state_summary(state: RemoteAccessState) -> &'static str {
    match state {
        RemoteAccessState::Disabled => "off",
        RemoteAccessState::Legacy => "on the older tunnel (the relay can read the traffic)",
        RemoteAccessState::Connecting => "connecting",
        RemoteAccessState::Enrolling => "setting up",
        RemoteAccessState::NeedsJoin => "needs to join another of your instances",
        RemoteAccessState::WaitingForDns => "waiting for DNS",
        RemoteAccessState::Ordering => "getting a certificate",
        RemoteAccessState::Ready => "ready",
        RemoteAccessState::Refused => "refused the relay's identity",
        RemoteAccessState::Error => "needs attention",
    }
}

async fn saved(client: &HubClient) -> Result<(), FatalError> {
    client
        .send_no_content(Method::POST, "/api/hub/remote-access/recovery-code/saved")
        .await?;
    println!("Done. Residuum no longer keeps the recovery code.");
    Ok(())
}

async fn reset_pins(client: &HubClient, recovery_code: Option<&str>) -> Result<(), FatalError> {
    let code = match recovery_code {
        Some(code) => code.to_string(),
        None => rpassword::prompt_password("recovery code: ").map_err(|e| {
            FatalError::Other(anyhow::anyhow!("Couldn't read the recovery code: {e}"))
        })?,
    };
    let body = serde_json::json!({ "recovery_code": code.trim() });
    client
        .send_no_content_with(Method::POST, "/api/hub/remote-access/reset-pins", &body)
        .await?;
    println!(
        "Your address now belongs to this instance's certificate account. Run `residuum remote status` to see the new recovery code and save it."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minted(codes: Option<Vec<String>>) -> PairLinkResponse {
        PairLinkResponse {
            link: "https://bear.agent-residuum.com/pair#token=abc".to_string(),
            qr_svg: String::new(),
            expires_in_secs: 600,
            recovery_codes: codes,
        }
    }

    #[test]
    fn the_link_and_its_lifetime_are_printed() {
        let text = pairing_instructions(&minted(None), None);
        assert!(text.contains("https://bear.agent-residuum.com/pair#token=abc"));
        assert!(text.contains("expires in 10 minutes"));
        assert!(!text.contains("Recovery codes"));
    }

    #[test]
    fn recovery_codes_are_printed_with_a_warning_when_present() {
        let text =
            pairing_instructions(&minted(Some(vec!["ABCD-EFGH-IJKL-MNOP".to_string()])), None);
        assert!(text.contains("ABCD-EFGH-IJKL-MNOP"));
        assert!(text.contains("only this once"));
    }

    #[test]
    fn a_qr_code_is_printed_when_one_could_be_drawn() {
        let text = pairing_instructions(&minted(None), Some("█▀█"));
        assert!(text.contains("█▀█"));
    }

    #[test]
    fn the_status_report_shows_addresses_warnings_and_the_recovery_code() {
        use residuum::remote_access::status::{PinInfo, RemoteHosts};
        let mut status = RemoteAccessStatus::new(RemoteAccessState::Ready);
        status.hosts = Some(RemoteHosts {
            ui: "bear.agent-residuum.com".into(),
            workbench: "bear.workbench.agent-residuum.com".into(),
            instance: "laptop.bear.agent-residuum.com".into(),
        });
        status.pins = vec![PinInfo {
            account_uri: "https://acme.test/acct/9".into(),
            slug: "stranger".into(),
            own: false,
            known: false,
        }];
        status.recovery_code = Some("ABCDEFGHIJKLMNOPQRST".into());
        let text = status_report(&status);
        assert!(text.contains("Remote access: ready"));
        assert!(text.contains("https://bear.agent-residuum.com"));
        assert!(text.contains("WARNING"));
        assert!(text.contains("stranger"));
        assert!(text.contains("ABCDEFGHIJKLMNOPQRST"));
        assert!(text.contains("residuum remote saved"));
    }

    #[test]
    fn an_empty_device_list_says_how_to_pair() {
        let listing = DeviceListResponse {
            devices: Vec::new(),
            pending: Vec::new(),
            recovery_codes_remaining: 0,
            ui_origin: None,
            remote: false,
        };
        assert!(device_table(&listing).contains("residuum remote pair"));
    }
}
