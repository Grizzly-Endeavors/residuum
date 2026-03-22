//! Connect subcommand: CLI client bridging stdin/stdout to a running gateway.

use residuum::gateway::protocol::{ClientMessage, ServerMessage};
use residuum::interfaces::cli::CliClient;
use residuum::interfaces::cli::commands::CommandEffect;
use residuum::util::FatalError;

/// Run the CLI connect client.
///
/// Connects to a running gateway over WebSocket and bridges stdin/stdout
/// to the agent.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the WebSocket connection fails.
pub(super) async fn run_connect(url: &str, verbose: bool) -> Result<(), FatalError> {
    use futures_util::StreamExt;
    use residuum::interfaces::cli::CliReader;

    let (ws_stream, _response) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to connect to {url}: {e}")))?;

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
        Err(e) => println!("error initializing readline: {e}"),
    });

    let mut msg_counter: u64 = 0;
    let mut indicator_tick = tokio::time::interval(std::time::Duration::from_millis(300));
    indicator_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Track whether we need to unblock the readline gate after the current turn
    let mut turn_active = false;

    loop {
        tokio::select! {
            // User input → check for commands, then send to gateway
            input = input_rx.recv() => {
                let Some(line) = input else {
                    println!("\nGoodbye!");
                    break;
                };

                match handle_cli_input(
                    &line, &mut client, &mut ws_tx, &gate_tx,
                    &mut msg_counter, &mut turn_active,
                ).await? {
                    std::ops::ControlFlow::Break(()) => break,
                    std::ops::ControlFlow::Continue(()) => {}
                }
            }

            // Gateway → display to user
            frame = ws_rx.next() => {
                let Some(frame_result) = frame else {
                    println!("connection closed by server");
                    break;
                };

                match handle_ws_frame(frame_result, &mut client, &mut turn_active, &gate_tx) {
                    std::ops::ControlFlow::Break(()) => break,
                    std::ops::ControlFlow::Continue(()) => {}
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

fn end_turn(
    client: &mut CliClient,
    msg: &ServerMessage,
    turn_active: &mut bool,
    gate_tx: &std::sync::mpsc::Sender<()>,
) {
    client.display(msg);
    *turn_active = false;
    if gate_tx.send(()).is_err() {
        tracing::debug!("prompt gate send failed: readline thread has exited");
    }
}

/// Process a single WebSocket frame from the gateway.
///
/// Returns `Break(())` to exit the event loop, `Continue(())` otherwise.
fn handle_ws_frame(
    frame_result: Result<
        tokio_tungstenite::tungstenite::Message,
        tokio_tungstenite::tungstenite::Error,
    >,
    client: &mut CliClient,
    turn_active: &mut bool,
    gate_tx: &std::sync::mpsc::Sender<()>,
) -> std::ops::ControlFlow<()> {
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    let raw = match frame_result {
        Ok(TungsteniteMessage::Text(txt)) => txt,
        Ok(TungsteniteMessage::Close(_)) => {
            println!("server closed connection");
            return std::ops::ControlFlow::Break(());
        }
        Ok(_) => return std::ops::ControlFlow::Continue(()),
        Err(e) => {
            println!("websocket error: {e}");
            return std::ops::ControlFlow::Break(());
        }
    };

    match serde_json::from_str::<ServerMessage>(&raw) {
        Ok(ServerMessage::Reloading) => {
            println!("server is reloading configuration...");
        }
        Ok(ref server_msg @ ServerMessage::Response { ref reply_to, .. })
            if *turn_active && !reply_to.is_empty() =>
        {
            end_turn(client, server_msg, turn_active, gate_tx);
        }
        Ok(ref server_msg @ ServerMessage::Error { .. }) if *turn_active => {
            end_turn(client, server_msg, turn_active, gate_tx);
        }
        Ok(server_msg) => client.display(&server_msg),
        Err(e) => tracing::warn!(error = %e, "failed to parse server message"),
    }

    std::ops::ControlFlow::Continue(())
}

/// Handle a line of CLI input: dispatch slash commands or send as a message.
///
/// Returns `Break(())` to exit the event loop, `Continue(())` otherwise.
///
/// # Errors
///
/// Returns `FatalError::Gateway` on serialization or send failure.
async fn handle_cli_input<S>(
    line: &str,
    client: &mut CliClient,
    ws_tx: &mut S,
    gate_tx: &std::sync::mpsc::Sender<()>,
    msg_counter: &mut u64,
    turn_active: &mut bool,
) -> Result<std::ops::ControlFlow<()>, FatalError>
where
    S: futures_util::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + Unpin,
{
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    if let Some(effect) = client.handle_command(line) {
        match effect {
            CommandEffect::ToggleVerbose => {
                let new_verbose = !client.verbose();
                client.set_verbose(new_verbose);
                let label = if new_verbose { "on" } else { "off" };
                println!("verbose mode: {label}");
                send_client_message(
                    ws_tx,
                    &ClientMessage::SetVerbose {
                        enabled: new_verbose,
                    },
                )
                .await?;
            }
            CommandEffect::Reload => {
                send_client_message(ws_tx, &ClientMessage::Reload).await?;
            }
            CommandEffect::ServerCommand { name, args } => {
                send_client_message(
                    ws_tx,
                    &ClientMessage::ServerCommand {
                        name: name.to_string(),
                        args,
                    },
                )
                .await?;
            }
            CommandEffect::InboxAdd(body) => {
                send_client_message(ws_tx, &ClientMessage::InboxAdd { body }).await?;
            }
            CommandEffect::Quit => return Ok(std::ops::ControlFlow::Break(())),
            CommandEffect::PrintLocal(text) => println!("{text}"),
        }
        // Slash commands don't trigger agent turns; unblock prompt immediately
        if gate_tx.send(()).is_err() {
            tracing::debug!("prompt gate send failed: readline thread has exited");
        }
        return Ok(std::ops::ControlFlow::Continue(()));
    }

    *msg_counter += 1;
    let client_msg = ClientMessage::SendMessage {
        id: format!("cli-{}", *msg_counter),
        content: line.to_string(),
        images: vec![],
    };
    let json = serde_json::to_string(&client_msg)
        .map_err(|e| FatalError::Gateway(format!("failed to serialize message: {e}")))?;
    if let Err(e) = ws_tx.send(TungsteniteMessage::text(json)).await {
        tracing::warn!(error = %e, "connection closed");
        return Ok(std::ops::ControlFlow::Break(()));
    }
    *turn_active = true;

    Ok(std::ops::ControlFlow::Continue(()))
}

/// Serialize and send a `ClientMessage` over the WebSocket.
///
/// # Errors
///
/// Returns `FatalError::Gateway` on serialization or send failure.
async fn send_client_message<S>(ws_tx: &mut S, msg: &ClientMessage) -> Result<(), FatalError>
where
    S: futures_util::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + Unpin,
{
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    let json = serde_json::to_string(msg)
        .map_err(|e| FatalError::Gateway(format!("failed to serialize message: {e}")))?;
    ws_tx
        .send(TungsteniteMessage::text(json))
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to send message: {e}")))?;
    Ok(())
}
