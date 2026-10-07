//! `remote` subcommand: pair browsers for remote access through Residuum Cloud.
//!
//! Every subcommand is a client of the running hub's pairing routes. The first
//! device is paired from the machine Residuum runs on, never over the relay,
//! so a server used only remotely is paired over SSH with `residuum remote pair`.

use std::fmt::Write as _;

use reqwest::Method;

use residuum::pairing::qr;
use residuum::pairing::types::{DeviceListResponse, PairLinkResponse};
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
