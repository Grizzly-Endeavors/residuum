//! `IronClaw`: personal AI agent gateway.
//!
//! Entrypoint with two subcommands:
//! - `serve` (default): starts the WebSocket gateway server
//! - `connect [url]`: connects a CLI client to a running gateway

use ironclaw::channels::cli::CliClient;
use ironclaw::channels::cli::commands::CommandEffect;
use ironclaw::config::Config;
use ironclaw::error::IronclawError;
use ironclaw::gateway::protocol::{ClientMessage, ServerMessage};

#[tokio::main]
async fn main() {
    if let Err(e) = Box::pin(run()).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "sequential setup/serve/connect dispatch with reload loop; splitting would obscure the flow"
)]
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
            // --setup: run the onboarding wizard in an isolated temp directory,
            // then boot the gateway with the resulting config for end-to-end testing
            if args.iter().any(|a| a == "--setup") {
                let tmp_dir = std::env::temp_dir().join("ironclaw-setup");
                if tmp_dir.exists() {
                    std::fs::remove_dir_all(&tmp_dir).map_err(|e| {
                        IronclawError::Config(format!(
                            "failed to clean setup directory {}: {e}",
                            tmp_dir.display()
                        ))
                    })?;
                }
                ironclaw::config::Config::bootstrap_at_dir(&tmp_dir)?;
                eprintln!(
                    "setup mode: config will be written to {}",
                    tmp_dir.display()
                );
                match Box::pin(ironclaw::gateway::server::setup::run_setup_server_at(
                    tmp_dir.clone(),
                ))
                .await?
                {
                    ironclaw::gateway::server::setup::SetupExit::ConfigSaved => {
                        tracing::info!("setup complete, loading config from temp directory");
                    }
                    ironclaw::gateway::server::setup::SetupExit::Shutdown => return Ok(()),
                }

                // Load the config written by the wizard and run the gateway
                loop {
                    let mut cfg = Config::load_at(&tmp_dir)?;
                    cfg.workspace_dir = tmp_dir.join("workspace");
                    tracing::info!(
                        model = %cfg.main.model,
                        provider_url = %cfg.main.provider_url,
                        workspace = %cfg.workspace_dir.display(),
                        "setup-mode: configuration loaded, starting gateway"
                    );
                    match Box::pin(ironclaw::gateway::server::run_gateway(cfg)).await? {
                        ironclaw::gateway::server::GatewayExit::Shutdown => break,
                        ironclaw::gateway::server::GatewayExit::Reload => {
                            tracing::info!("configuration reloaded, restarting gateway");
                        }
                    }
                }
                return Ok(());
            }

            loop {
                Config::bootstrap_config_dir()?;
                match Config::load() {
                    Ok(cfg) => {
                        tracing::info!(
                            model = %cfg.main.model,
                            provider_url = %cfg.main.provider_url,
                            workspace = %cfg.workspace_dir.display(),
                            "configuration loaded"
                        );
                        match Box::pin(ironclaw::gateway::server::run_gateway(cfg)).await? {
                            ironclaw::gateway::server::GatewayExit::Shutdown => break,
                            ironclaw::gateway::server::GatewayExit::Reload => {
                                tracing::info!("configuration reloaded, restarting gateway");
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "config invalid, starting setup wizard");
                        match Box::pin(ironclaw::gateway::server::setup::run_setup_server()).await?
                        {
                            ironclaw::gateway::server::setup::SetupExit::ConfigSaved => {
                                tracing::info!("setup complete, loading configuration");
                            }
                            ironclaw::gateway::server::setup::SetupExit::Shutdown => break,
                        }
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
#[expect(
    clippy::too_many_lines,
    reason = "CLI connect loop wires up readline, WS, and indicator; splitting would obscure the event flow"
)]
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

    // Prompt gate: readline blocks after sending input until we signal turn completion
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let prompt = client.user_prompt();

    // Spawn readline thread
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(1);
    tokio::task::spawn_blocking(move || match CliReader::new() {
        Ok(reader) => reader.run(input_tx, prompt, gate_rx),
        Err(e) => eprintln!("error initializing readline: {e}"),
    });

    let mut msg_counter: u64 = 0;
    let mut indicator_tick = tokio::time::interval(std::time::Duration::from_millis(300));
    // Track whether we need to unblock the readline gate after the current turn
    let mut turn_active = false;

    loop {
        tokio::select! {
            // User input → check for commands, then send to gateway
            input = input_rx.recv() => {
                let Some(line) = input else {
                    eprintln!("\nGoodbye!");
                    break;
                };

                // Check for slash commands
                if let Some(effect) = client.handle_command(&line) {
                    match effect {
                        CommandEffect::ToggleVerbose => {
                            let new_verbose = !client.verbose();
                            client.set_verbose(new_verbose);
                            let label = if new_verbose { "on" } else { "off" };
                            eprintln!("verbose mode: {label}");
                            send_client_message(
                                &mut ws_tx,
                                &ClientMessage::SetVerbose { enabled: new_verbose },
                            ).await?;
                        }
                        CommandEffect::Reload => {
                            send_client_message(&mut ws_tx, &ClientMessage::Reload).await?;
                        }
                        CommandEffect::ServerCommand { name, args } => {
                            send_client_message(
                                &mut ws_tx,
                                &ClientMessage::ServerCommand {
                                    name: name.to_string(),
                                    args,
                                },
                            ).await?;
                        }
                        CommandEffect::InboxAdd(body) => {
                            send_client_message(
                                &mut ws_tx,
                                &ClientMessage::InboxAdd { body },
                            )
                            .await?;
                        }
                        CommandEffect::Quit => break,
                        CommandEffect::PrintLocal(text) => eprintln!("{text}"),
                    }
                    // Slash commands don't trigger agent turns; unblock prompt immediately
                    gate_tx.send(()).ok();
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
                turn_active = true;
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
                    Ok(ref server_msg @ ServerMessage::Response { ref reply_to, .. })
                        if turn_active && !reply_to.is_empty() =>
                    {
                        client.display(server_msg);
                        // Final response (non-empty reply_to) ends the turn
                        turn_active = false;
                        gate_tx.send(()).ok();
                    }
                    Ok(ref server_msg @ ServerMessage::Error { .. }) if turn_active => {
                        client.display(server_msg);
                        turn_active = false;
                        gate_tx.send(()).ok();
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
