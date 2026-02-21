//! `IronClaw`: personal AI agent gateway.
//!
//! Entrypoint with two subcommands:
//! - `serve` (default): starts the WebSocket gateway server
//! - `connect [url]`: connects a CLI client to a running gateway

use ironclaw::cli::CliClient;
use ironclaw::cli::commands::{CommandAction, SlashCommand};
use ironclaw::config::Config;
use ironclaw::error::IronclawError;
use ironclaw::gateway::protocol::{ClientMessage, ServerMessage};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), IronclawError> {
    // Init tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    // Load .env (ignore if missing, warn on parse errors)
    if let Err(e) = dotenvy::dotenv()
        && !e.not_found()
    {
        tracing::warn!(error = %e, "failed to parse .env file");
    }

    // Parse subcommand from argv
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(String::as_str);

    match subcommand {
        Some("connect") => {
            let url = args.get(2).map_or("ws://127.0.0.1:7700/ws", String::as_str);
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            run_connect(url, verbose).await
        }
        // "serve" or no subcommand → start gateway
        Some("serve") | None => {
            loop {
                Config::bootstrap_config_dir()?;
                let cfg = Config::load()?;
                tracing::info!(
                    model = %cfg.main.model,
                    provider_url = %cfg.main.provider_url,
                    workspace = %cfg.workspace_dir.display(),
                    "configuration loaded"
                );
                match ironclaw::gateway::server::run_gateway(cfg).await? {
                    ironclaw::gateway::server::GatewayExit::Shutdown => break,
                    ironclaw::gateway::server::GatewayExit::Reload => {
                        tracing::info!("configuration reloaded, restarting gateway");
                    }
                }
            }
            Ok(())
        }
        Some(other) => Err(IronclawError::Config(format!(
            "unknown subcommand '{other}', expected 'serve' or 'connect'"
        ))),
    }
}

/// Run the CLI connect client.
///
/// Connects to a running gateway over WebSocket and bridges stdin/stdout
/// to the agent.
///
/// # Errors
///
/// Returns `IronclawError::Gateway` if the WebSocket connection fails.
async fn run_connect(url: &str, verbose: bool) -> Result<(), IronclawError> {
    use futures_util::{SinkExt, StreamExt};
    use ironclaw::channels::cli::CliReader;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    let (ws_stream, _response) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| IronclawError::Gateway(format!("failed to connect to {url}: {e}")))?;

    let mut client = CliClient::new(url, verbose);
    client.print_banner();

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Send verbose preference if requested
    if verbose {
        send_client_message(&mut ws_tx, &ClientMessage::SetVerbose { enabled: true }).await?;
    }

    // Spawn readline thread
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(1);
    tokio::task::spawn_blocking(move || match CliReader::new() {
        Ok(reader) => reader.run(input_tx),
        Err(e) => eprintln!("error initializing readline: {e}"),
    });

    let mut msg_counter: u64 = 0;
    let mut indicator_tick = tokio::time::interval(std::time::Duration::from_millis(300));

    loop {
        tokio::select! {
            // User input → check for commands, then send to gateway
            input = input_rx.recv() => {
                let Some(line) = input else {
                    eprintln!("\nGoodbye!");
                    break;
                };

                // Check for slash commands
                if let Some(cmd) = SlashCommand::parse(&line) {
                    match client.handle_command(&cmd) {
                        CommandAction::ToggleVerbose => {
                            let new_verbose = !client.verbose();
                            client.set_verbose(new_verbose);
                            let label = if new_verbose { "on" } else { "off" };
                            eprintln!("verbose mode: {label}");
                            send_client_message(
                                &mut ws_tx,
                                &ClientMessage::SetVerbose { enabled: new_verbose },
                            ).await?;
                        }
                        CommandAction::Reload => {
                            send_client_message(&mut ws_tx, &ClientMessage::Reload).await?;
                            // The server will broadcast Reloading then drop connections;
                            // we break here and let the ws_rx arm print the notice.
                        }
                        CommandAction::Quit => break,
                        CommandAction::PrintOutput(text) => eprintln!("{text}"),
                        CommandAction::None => {}
                    }
                    continue;
                }

                msg_counter = msg_counter.wrapping_add(1);
                let client_msg = ClientMessage::SendMessage {
                    id: format!("cli-{msg_counter}"),
                    content: line,
                };
                let json = serde_json::to_string(&client_msg).map_err(|e| {
                    IronclawError::Gateway(format!("failed to serialize message: {e}"))
                })?;
                if ws_tx.send(TungsteniteMessage::text(json)).await.is_err() {
                    eprintln!("connection closed");
                    break;
                }
            }

            // Gateway → display to user
            frame = ws_rx.next() => {
                let Some(frame_result) = frame else {
                    eprintln!("connection closed by server");
                    break;
                };

                let raw = match frame_result {
                    Ok(TungsteniteMessage::Text(txt)) => txt,
                    Ok(TungsteniteMessage::Close(_)) => {
                        eprintln!("server closed connection");
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        eprintln!("websocket error: {e}");
                        break;
                    }
                };

                match serde_json::from_str::<ServerMessage>(&raw) {
                    Ok(ServerMessage::Reloading) => {
                        eprintln!("server is reloading, reconnect when ready");
                        break;
                    }
                    Ok(server_msg) => client.display(&server_msg),
                    Err(e) => eprintln!("warning: failed to parse server message: {e}"),
                }
            }

            // Indicator animation tick
            _ = indicator_tick.tick(), if client.indicator.is_active() => {
                client.indicator.tick();
            }
        }
    }

    Ok(())
}

/// Serialize and send a `ClientMessage` over the WebSocket.
///
/// # Errors
///
/// Returns `IronclawError::Gateway` on serialization or send failure.
async fn send_client_message<S>(ws_tx: &mut S, msg: &ClientMessage) -> Result<(), IronclawError>
where
    S: futures_util::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + Unpin,
{
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    let json = serde_json::to_string(msg)
        .map_err(|e| IronclawError::Gateway(format!("failed to serialize message: {e}")))?;
    ws_tx
        .send(TungsteniteMessage::text(json))
        .await
        .map_err(|e| IronclawError::Gateway(format!("failed to send message: {e}")))?;
    Ok(())
}
