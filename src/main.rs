//! `IronClaw`: personal AI agent gateway.
//!
//! Entrypoint with two subcommands:
//! - `serve` (default): starts the WebSocket gateway server
//! - `connect [url]`: connects a CLI client to a running gateway

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
            let cfg = Config::load()?;
            tracing::info!(
                model = %cfg.model,
                provider_url = %cfg.provider_url,
                workspace = %cfg.workspace_dir.display(),
                "configuration loaded"
            );
            ironclaw::gateway::server::run_gateway(cfg).await
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

    eprintln!("connected to {url}");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Send verbose preference if requested
    if verbose {
        let msg = ClientMessage::SetVerbose { enabled: true };
        let json = serde_json::to_string(&msg)
            .map_err(|e| IronclawError::Gateway(format!("failed to serialize set_verbose: {e}")))?;
        ws_tx
            .send(TungsteniteMessage::text(json))
            .await
            .map_err(|e| IronclawError::Gateway(format!("failed to send set_verbose: {e}")))?;
    }

    // Spawn readline thread
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(1);
    tokio::task::spawn_blocking(move || match CliReader::new() {
        Ok(reader) => reader.run(input_tx),
        Err(e) => eprintln!("error initializing readline: {e}"),
    });

    let mut msg_counter: u64 = 0;

    loop {
        tokio::select! {
            // User input → send to gateway
            input = input_rx.recv() => {
                let Some(line) = input else {
                    eprintln!("\nGoodbye!");
                    break;
                };

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
                    Ok(server_msg) => display_server_message(&server_msg),
                    Err(e) => eprintln!("warning: failed to parse server message: {e}"),
                }
            }
        }
    }

    Ok(())
}

/// Display a server message in the CLI.
fn display_server_message(msg: &ServerMessage) {
    match msg {
        ServerMessage::TurnStarted { .. } | ServerMessage::Pong => {
            // No-op — TurnStarted is informational, Pong is keepalive
        }
        ServerMessage::ToolCall { name, arguments } => {
            eprintln!("[tool: {name}] {arguments}");
        }
        ServerMessage::ToolResult {
            name,
            output,
            is_error,
        } => {
            if *is_error {
                eprintln!("[tool: {name} ERROR] {output}");
            } else {
                let preview = if output.len() > 200 {
                    format!(
                        "{}... ({} bytes)",
                        output.get(..200).unwrap_or(output),
                        output.len()
                    )
                } else {
                    output.clone()
                };
                eprintln!("[tool: {name}] {preview}");
            }
        }
        ServerMessage::Response { content, .. } => {
            println!("ironclaw: {content}");
        }
        ServerMessage::SystemEvent { source, content } => {
            println!("\n[{source}] {content}\n");
        }
        ServerMessage::Error { message, .. } => {
            eprintln!("error: {message}");
        }
    }
}
