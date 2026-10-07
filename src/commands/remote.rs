//! `remote` subcommand: pair browsers for remote access through Residuum Cloud,
//! and join this instance to the user's other instances.
//!
//! Every subcommand is a client of the running hub's pairing and remote access routes. The first
//! device is paired from the machine Residuum runs on, never over the relay,
//! so a server used only remotely is paired over SSH with `residuum remote pair`.

use std::fmt::Write as _;

use reqwest::Method;

use residuum::pairing::qr;
use residuum::pairing::types::{DeviceListResponse, PairLinkResponse};
use residuum::remote_access::status::{JoinState, RemoteAccessState, RemoteAccessStatus};
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
    /// Ask another of your instances to approve this one, so it can get a certificate and call it
    Join {
        /// The other instance's name, as shown in Residuum Cloud (for example `laptop`)
        instance: String,
    },
    /// List the instances asking this one to approve them, with the codes to compare
    Joins,
    /// Approve an instance asking to join, after checking its code matches
    Approve {
        /// The request's id, from `residuum remote joins`
        id: String,
    },
    /// Refuse an instance asking to join
    Deny {
        /// The request's id, from `residuum remote joins`
        id: String,
    },
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
        RemoteCommand::Join { instance } => join(&client, instance).await,
        RemoteCommand::Joins => joins(&client).await,
        RemoteCommand::Approve { id } => decide(&client, id, "approve").await,
        RemoteCommand::Deny { id } => decide(&client, id, "deny").await,
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
    if !status.siblings.is_empty() {
        out.push_str("\nJoined instances (they call each other's agents):\n");
        for sibling in &status.siblings {
            _ = writeln!(out, "  {}", sibling.slug);
        }
    }
    let removable: Vec<_> = status.pins.iter().filter(|pin| pin.removable).collect();
    if !removable.is_empty() {
        out.push_str(
            "\nThese certificate accounts belong to instances Residuum Cloud no longer lists. Remove them in Settings, Remote access:\n",
        );
        for pin in removable {
            _ = writeln!(out, "  {} (instance {})", pin.account_uri, pin.slug);
        }
    }
    if !status.pending_joins.is_empty() {
        _ = writeln!(
            out,
            "\n{} instance(s) are asking to join. See `residuum remote joins`.",
            status.pending_joins.len()
        );
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

/// How often `residuum remote join` looks at the join's progress.
const JOIN_POLL: std::time::Duration = std::time::Duration::from_secs(2);

async fn join(client: &HubClient, instance: &str) -> Result<(), FatalError> {
    let body = serde_json::json!({ "instance": instance.trim() });
    client
        .send_no_content_with(Method::POST, "/api/hub/remote-access/join", &body)
        .await?;
    println!("Asking \"{}\" to approve this instance...", instance.trim());
    let mut shown_code = false;
    loop {
        tokio::time::sleep(JOIN_POLL).await;
        let status: RemoteAccessStatus = client
            .send(Method::GET, "/api/hub/remote-access/status", None)
            .await?;
        let Some(progress) = status.join.filter(|p| p.instance == instance.trim()) else {
            continue;
        };
        if let Some(code) = &progress.code
            && !shown_code
        {
            shown_code = true;
            println!(
                "\nThe code is {code}. On \"{}\", open Settings, Remote access (or run `residuum remote joins`) and approve the request only if it shows the same code.\nWaiting for the approval...",
                instance.trim()
            );
        }
        match progress.state {
            JoinState::Waiting => {}
            JoinState::Approved => {
                println!(
                    "\nApproved. This instance is now a sibling of \"{}\".",
                    instance.trim()
                );
                println!("Run `residuum remote status` to follow the certificate.");
                return Ok(());
            }
            JoinState::Denied | JoinState::Failed => {
                return Err(FatalError::Other(anyhow::anyhow!(
                    progress
                        .detail
                        .unwrap_or_else(|| "The join did not complete.".to_string())
                )));
            }
        }
    }
}

async fn joins(client: &HubClient) -> Result<(), FatalError> {
    let status: RemoteAccessStatus = client
        .send(Method::GET, "/api/hub/remote-access/status", None)
        .await?;
    println!("{}", join_requests(&status));
    Ok(())
}

/// What `residuum remote joins` prints.
fn join_requests(status: &RemoteAccessStatus) -> String {
    if status.pending_joins.is_empty() {
        return "No instance is asking to join.".to_string();
    }
    let mut out = String::from(
        "Instances asking to join. Approve one only if its code matches the code shown on that instance:\n",
    );
    for request in &status.pending_joins {
        let hint = match request.in_relay_list {
            Some(true) => "listed by Residuum Cloud",
            Some(false) => "NOT listed by Residuum Cloud",
            None => "Residuum Cloud's list is unknown",
        };
        _ = writeln!(
            out,
            "  {}  code {}  instance \"{}\" ({}) {hint}",
            request.id,
            request.code,
            request.slug,
            printable(&request.display_name),
        );
    }
    out.push_str(
        "\nApprove with `residuum remote approve <id>`, refuse with `residuum remote deny <id>`.",
    );
    out
}

/// Text from another instance with anything that could rewrite a terminal line removed.
fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

async fn decide(client: &HubClient, id: &str, verb: &str) -> Result<(), FatalError> {
    let path = format!("/api/hub/remote-access/joins/{}/{verb}", id.trim());
    client.send_no_content(Method::POST, &path).await?;
    if verb == "approve" {
        println!(
            "Approved. That instance can now get certificates for your addresses and call your agents."
        );
    } else {
        println!("Denied.");
    }
    Ok(())
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
            removable: false,
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
    fn join_requests_show_codes_hints_and_how_to_decide() {
        use residuum::remote_access::status::PendingJoinInfo;
        let mut status = RemoteAccessStatus::new(RemoteAccessState::Ready);
        assert!(join_requests(&status).contains("No instance is asking"));
        status.pending_joins = vec![
            PendingJoinInfo {
                id: "ab12cd34ef56".into(),
                code: "482913".into(),
                slug: "desktop".into(),
                display_name: "Desk\u{1b}[2J top".into(),
                in_relay_list: Some(true),
                expires_at: "2026-01-01T00:00:00Z".into(),
            },
            PendingJoinInfo {
                id: "ffffffffffff".into(),
                code: "000111".into(),
                slug: "ghost".into(),
                display_name: "Ghost".into(),
                in_relay_list: Some(false),
                expires_at: "2026-01-01T00:00:00Z".into(),
            },
        ];
        let text = join_requests(&status);
        assert!(text.contains("ab12cd34ef56  code 482913"));
        assert!(text.contains("listed by Residuum Cloud"));
        assert!(text.contains("NOT listed by Residuum Cloud"));
        assert!(
            !text.contains('\u{1b}'),
            "control characters from another instance never reach the terminal"
        );
        assert!(text.contains("residuum remote approve"));
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
