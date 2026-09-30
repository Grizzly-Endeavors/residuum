//! Agent subcommand: list, create, delete and control the hub's agents.
//!
//! Every subcommand is a client of the running hub's `/api/hub/agents`
//! routes; the hub owns the lifecycle, so nothing here touches disk.

use std::fmt::Write as _;

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use residuum::config::paths::validate_agent_name;
use residuum::util::FatalError;

use super::hub_client::HubClient;

#[derive(clap::Subcommand)]
pub(super) enum AgentCommand {
    /// List agents with their state, autostart setting and role
    List,
    /// Create an agent
    Create(CreateArgs),
    /// Delete an agent (its files are checkpointed first)
    Delete {
        /// Name of the agent to delete
        name: String,
    },
    /// Start a stopped or failed agent
    Start {
        /// Name of the agent to start
        name: String,
    },
    /// Stop a running agent
    Stop {
        /// Name of the agent to stop
        name: String,
    },
    /// Restart an agent
    Restart {
        /// Name of the agent to restart
        name: String,
    },
    /// Turn an agent's start-with-Residuum setting on or off
    Autostart {
        /// Name of the agent
        name: String,
        /// Whether the agent starts when Residuum starts
        #[arg(value_enum)]
        setting: AutostartSetting,
    },
}

#[derive(clap::Args)]
pub(super) struct CreateArgs {
    /// Name for the new agent (lowercase letters, digits and hyphens)
    name: String,
    /// What the agent is for; it receives this as its first message
    #[arg(long)]
    description: Option<String>,
    /// Copy model settings from this existing agent
    #[arg(long, value_name = "AGENT")]
    models_from: Option<String>,
    /// Let other agents and callers discover this agent over A2A
    #[arg(long)]
    public: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AutostartSetting {
    On,
    Off,
}

/// Mirrors the contract's `AgentSummary`.
#[derive(Deserialize)]
struct AgentSummary {
    name: String,
    state: String,
    #[serde(default)]
    last_error: Option<LastError>,
    autostart: bool,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    a2a_visibility: Option<String>,
}

#[derive(Deserialize)]
struct LastError {
    message: String,
    #[serde(default)]
    at: Option<String>,
}

#[derive(Deserialize)]
struct AgentList {
    agents: Vec<AgentSummary>,
}

#[derive(Deserialize)]
struct DeleteResponse {
    #[serde(default)]
    checkpoint_id: Option<String>,
}

/// Run an `agent` subcommand against the hub and print its output.
///
/// # Errors
///
/// Returns `FatalError` with a plain-language message when the hub isn't
/// running, rejects the request, or the arguments are invalid.
#[tracing::instrument(skip_all)]
pub(super) async fn run_agent_command(
    command: &AgentCommand,
    gateway_addr: &str,
) -> Result<(), FatalError> {
    let output = execute(command, gateway_addr).await?;
    println!("{output}");
    Ok(())
}

async fn execute(command: &AgentCommand, gateway_addr: &str) -> Result<String, FatalError> {
    let client = HubClient::new(gateway_addr)?;
    match command {
        AgentCommand::List => list(&client).await,
        AgentCommand::Create(args) => create(&client, args).await,
        AgentCommand::Delete { name } => delete(&client, name).await,
        AgentCommand::Start { name } => transition(&client, name, "start").await,
        AgentCommand::Stop { name } => transition(&client, name, "stop").await,
        AgentCommand::Restart { name } => transition(&client, name, "restart").await,
        AgentCommand::Autostart { name, setting } => autostart(&client, name, *setting).await,
    }
}

fn check_name(name: &str) -> Result<(), FatalError> {
    validate_agent_name(name).map_err(|reason| {
        tracing::warn!(agent = name, reason = %reason, "rejected agent name");
        FatalError::Other(anyhow::anyhow!("{reason}"))
    })
}

async fn fetch_agents(client: &HubClient) -> Result<Vec<AgentSummary>, FatalError> {
    let list: AgentList = client.send(Method::GET, "/api/hub/agents", None).await?;
    Ok(list.agents)
}

async fn list(client: &HubClient) -> Result<String, FatalError> {
    let agents = fetch_agents(client).await?;
    Ok(render_table(&agents))
}

async fn create(client: &HubClient, args: &CreateArgs) -> Result<String, FatalError> {
    check_name(&args.name)?;
    let models_from = match &args.models_from {
        Some(source) => {
            check_name(source)?;
            source.clone()
        }
        None => default_models_source(&fetch_agents(client).await?)?,
    };

    let mut fields = serde_json::Map::new();
    fields.insert("name".into(), json!(args.name));
    fields.insert("models_from".into(), json!(models_from));
    fields.insert(
        "a2a_visibility".into(),
        json!(if args.public { "public" } else { "private" }),
    );
    if let Some(description) = &args.description {
        fields.insert("description".into(), json!(description));
    }
    let body = serde_json::Value::Object(fields);

    let created: AgentSummary = client
        .send(Method::POST, "/api/hub/agents", Some(&body))
        .await?;
    tracing::info!(agent = %created.name, models_from = %models_from, "created agent");
    Ok(format!(
        "Created agent '{}' ({}, {}). Model settings copied from '{models_from}'.",
        created.name,
        created.state,
        created.a2a_visibility.as_deref().unwrap_or("private"),
    ))
}

/// The agent to copy model settings from when `--models-from` is omitted:
/// the only running agent, if there is exactly one.
fn default_models_source(agents: &[AgentSummary]) -> Result<String, FatalError> {
    let mut running = agents.iter().filter(|a| a.state == "running");
    match (running.next(), running.next()) {
        (Some(only), None) => Ok(only.name.clone()),
        (None, _) => Err(FatalError::Other(anyhow::anyhow!(
            "No agent is running to copy model settings from. Start one, or pass `--models-from <agent>`."
        ))),
        (Some(_), Some(_)) => Err(FatalError::Other(anyhow::anyhow!(
            "More than one agent is running, so Residuum can't tell which model settings to copy. Pass `--models-from <agent>`."
        ))),
    }
}

async fn delete(client: &HubClient, name: &str) -> Result<String, FatalError> {
    check_name(name)?;
    let response: DeleteResponse = client
        .send(Method::DELETE, &format!("/api/hub/agents/{name}"), None)
        .await?;
    tracing::info!(agent = name, checkpoint_id = ?response.checkpoint_id, "deleted agent");
    Ok(match response.checkpoint_id {
        Some(id) => format!(
            "Deleted agent '{name}'.\nCheckpoint id: {id}\nThe agent's files are kept in the hub's checkpoint history and can be restored from it. The CLI has no restore command; use the checkpoint history in the web UI or the `workspace_restore` tool."
        ),
        None => format!("Deleted agent '{name}'. The server reported no checkpoint for it."),
    })
}

async fn transition(client: &HubClient, name: &str, action: &str) -> Result<String, FatalError> {
    check_name(name)?;
    let summary: AgentSummary = client
        .send(
            Method::POST,
            &format!("/api/hub/agents/{name}/{action}"),
            None,
        )
        .await?;
    tracing::info!(agent = name, action, state = %summary.state, "agent lifecycle request");
    let mut out = format!("Agent '{}' is {}.", summary.name, summary.state);
    if let Some(error) = &summary.last_error {
        _ = write!(out, "\nLast error: {}", error.message);
    }
    Ok(out)
}

async fn autostart(
    client: &HubClient,
    name: &str,
    setting: AutostartSetting,
) -> Result<String, FatalError> {
    check_name(name)?;
    let enabled = setting == AutostartSetting::On;
    let summary: AgentSummary = client
        .send(
            Method::PATCH,
            &format!("/api/hub/agents/{name}"),
            Some(&json!({ "autostart": enabled })),
        )
        .await?;
    tracing::info!(
        agent = name,
        autostart = summary.autostart,
        "updated autostart"
    );
    Ok(format!(
        "Agent '{}' will {} when Residuum starts.",
        summary.name,
        if summary.autostart {
            "start"
        } else {
            "not start"
        }
    ))
}

fn render_table(agents: &[AgentSummary]) -> String {
    if agents.is_empty() {
        return "No agents yet. Create one with `residuum agent create <name>`.".to_string();
    }

    let name_width = agents
        .iter()
        .map(|a| a.name.len())
        .max()
        .unwrap_or(0)
        .max(4);
    let state_width = agents
        .iter()
        .map(|a| a.state.len())
        .max()
        .unwrap_or(0)
        .max(5);

    let mut out = format!(
        "{:<name_width$}  {:<state_width$}  {:<9}  ROLE",
        "NAME", "STATE", "AUTOSTART"
    );
    for agent in agents {
        let autostart = if agent.autostart { "on" } else { "off" };
        let role = agent.role.as_deref().unwrap_or("-");
        _ = write!(
            out,
            "\n{:<name_width$}  {:<state_width$}  {autostart:<9}  {role}",
            agent.name, agent.state
        );
        if let Some(error) = &agent.last_error {
            let when = error
                .at
                .as_deref()
                .map_or_else(String::new, |at| format!(" ({at})"));
            _ = write!(out, "\n  last error: {}{when}", error.message);
        }
    }
    out
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes serde_json::Value and request logs by known-present keys"
)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{Value, json};

    use super::*;

    #[derive(Default)]
    struct Mock {
        agents: Vec<Value>,
        requests: Vec<(String, String, Value)>,
    }

    type Shared = Arc<Mutex<Mock>>;

    fn summary(name: &str, state: &str) -> Value {
        json!({
            "name": name,
            "state": state,
            "last_error": null,
            "autostart": true,
            "role": "researcher",
            "a2a_visibility": "private",
        })
    }

    fn record(shared: &Shared, method: &str, path: String, body: Value) {
        shared
            .lock()
            .unwrap()
            .requests
            .push((method.to_string(), path, body));
    }

    fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
        (status, Json(json!({ "error": message })))
    }

    async fn list_agents(State(s): State<Shared>) -> Json<Value> {
        record(&s, "GET", "/api/hub/agents".into(), Value::Null);
        Json(json!({ "agents": s.lock().unwrap().agents.clone() }))
    }

    async fn create_agent(
        State(s): State<Shared>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        record(&s, "POST", "/api/hub/agents".into(), body.clone());
        let name = body["name"].as_str().unwrap_or_default();
        if name == "taken" {
            return error(
                StatusCode::CONFLICT,
                "an agent named 'taken' already exists",
            );
        }
        if body["models_from"] == "ghost" {
            return error(StatusCode::NOT_FOUND, "no agent named 'ghost'");
        }
        let mut created = summary(name, "starting");
        created["a2a_visibility"] = body["a2a_visibility"].clone();
        (StatusCode::CREATED, Json(created))
    }

    async fn delete_agent(
        State(s): State<Shared>,
        Path(name): Path<String>,
    ) -> (StatusCode, Json<Value>) {
        record(&s, "DELETE", format!("/api/hub/agents/{name}"), Value::Null);
        match name.as_str() {
            "ghost" => error(StatusCode::NOT_FOUND, "no agent named 'ghost'"),
            "empty" => (
                StatusCode::OK,
                Json(json!({ "deleted": true, "checkpoint_id": null })),
            ),
            _ => (
                StatusCode::OK,
                Json(json!({ "deleted": true, "checkpoint_id": "abc123" })),
            ),
        }
    }

    async fn transition_agent(
        State(s): State<Shared>,
        Path((name, action)): Path<(String, String)>,
    ) -> (StatusCode, Json<Value>) {
        record(
            &s,
            "POST",
            format!("/api/hub/agents/{name}/{action}"),
            Value::Null,
        );
        match name.as_str() {
            "ghost" => error(StatusCode::NOT_FOUND, "no agent named 'ghost'"),
            "broken" => {
                let mut failed = summary("broken", "failed");
                failed["last_error"] =
                    json!({ "message": "bad config", "at": "2026-09-29T10:00:00Z" });
                (StatusCode::OK, Json(failed))
            }
            _ => {
                let state = if action == "stop" {
                    "stopped"
                } else {
                    "running"
                };
                (StatusCode::OK, Json(summary(&name, state)))
            }
        }
    }

    async fn patch_agent(
        State(s): State<Shared>,
        Path(name): Path<String>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        record(&s, "PATCH", format!("/api/hub/agents/{name}"), body.clone());
        if name == "ghost" {
            return error(StatusCode::NOT_FOUND, "no agent named 'ghost'");
        }
        let mut updated = summary(&name, "running");
        updated["autostart"] = body["autostart"].clone();
        (StatusCode::OK, Json(updated))
    }

    async fn bad_request() -> (StatusCode, Json<Value>) {
        error(StatusCode::BAD_REQUEST, "invalid agent name")
    }

    /// Start a mock hub implementing the contract's lifecycle routes.
    async fn mock_hub(agents: Vec<Value>) -> (String, Shared) {
        let shared: Shared = Arc::new(Mutex::new(Mock {
            agents,
            requests: Vec::new(),
        }));
        let app = Router::new()
            .route("/api/hub/agents", get(list_agents).post(create_agent))
            .route(
                "/api/hub/agents/{name}",
                axum::routing::delete(delete_agent).patch(patch_agent),
            )
            .route("/api/hub/agents/{name}/{action}", post(transition_agent))
            .with_state(Arc::clone(&shared));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, shared)
    }

    fn create_args(name: &str) -> CreateArgs {
        CreateArgs {
            name: name.to_string(),
            description: None,
            models_from: None,
            public: false,
        }
    }

    fn message(result: Result<String, FatalError>) -> String {
        match result {
            Ok(out) => out,
            Err(e) => e.to_string(),
        }
    }

    #[tokio::test]
    async fn list_renders_table_with_failed_agent_error() {
        let mut failed = summary("broken", "failed");
        failed["last_error"] = json!({ "message": "bad config", "at": "2026-09-29T10:00:00Z" });
        failed["autostart"] = json!(false);
        failed["role"] = Value::Null;
        let (addr, _) = mock_hub(vec![summary("scout", "running"), failed]).await;

        let out = execute(&AgentCommand::List, &addr).await.unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "NAME    STATE    AUTOSTART  ROLE");
        assert_eq!(lines[1], "scout   running  on         researcher");
        assert_eq!(lines[2], "broken  failed   off        -");
        assert_eq!(lines[3], "  last error: bad config (2026-09-29T10:00:00Z)");
    }

    #[tokio::test]
    async fn list_with_no_agents_points_at_create() {
        let (addr, _) = mock_hub(vec![]).await;
        let out = execute(&AgentCommand::List, &addr).await.unwrap();
        assert!(out.contains("residuum agent create"));
    }

    #[tokio::test]
    async fn hub_not_running_gives_plain_message() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);

        let out = message(execute(&AgentCommand::List, &addr).await);
        assert_eq!(
            out,
            "Residuum isn't running. Start it with `residuum serve`."
        );
    }

    #[tokio::test]
    async fn create_defaults_models_from_only_running_agent() {
        let (addr, mock) = mock_hub(vec![
            summary("scout", "running"),
            summary("idle", "stopped"),
        ])
        .await;

        let out = execute(&AgentCommand::Create(create_args("new-one")), &addr)
            .await
            .unwrap();
        assert!(out.contains("Created agent 'new-one'"));
        assert!(out.contains("copied from 'scout'"));

        let requests = mock.lock().unwrap().requests.clone();
        let (method, path, body) = requests.last().unwrap();
        assert_eq!(
            (method.as_str(), path.as_str()),
            ("POST", "/api/hub/agents")
        );
        assert_eq!(body["models_from"], "scout");
        assert_eq!(body["a2a_visibility"], "private");
        assert!(body.get("description").is_none());
    }

    #[tokio::test]
    async fn create_with_flags_sends_description_source_and_public() {
        let (addr, mock) = mock_hub(vec![summary("a", "running"), summary("b", "running")]).await;
        let mut args = create_args("new-one");
        args.description = Some("watches the feeds".to_string());
        args.models_from = Some("b".to_string());
        args.public = true;

        let out = execute(&AgentCommand::Create(args), &addr).await.unwrap();
        assert!(out.contains("public"));

        let requests = mock.lock().unwrap().requests.clone();
        // An explicit source needs no lookup of the running agents.
        assert_eq!(requests.len(), 1);
        let body = &requests[0].2;
        assert_eq!(body["description"], "watches the feeds");
        assert_eq!(body["models_from"], "b");
        assert_eq!(body["a2a_visibility"], "public");
    }

    #[tokio::test]
    async fn create_without_a_single_running_agent_asks_for_models_from() {
        let (addr, mock) = mock_hub(vec![summary("a", "running"), summary("b", "running")]).await;
        let many = message(execute(&AgentCommand::Create(create_args("new-one")), &addr).await);
        assert!(many.contains("--models-from"));
        assert!(many.contains("More than one"));

        let (idle_addr, _) = mock_hub(vec![summary("a", "stopped")]).await;
        let none =
            message(execute(&AgentCommand::Create(create_args("new-one")), &idle_addr).await);
        assert!(none.contains("--models-from"));
        assert!(none.contains("No agent is running"));

        let posts = mock
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(m, _, _)| m == "POST")
            .count();
        assert_eq!(posts, 0);
    }

    #[tokio::test]
    async fn create_rejects_invalid_name_before_contacting_hub() {
        let (addr, mock) = mock_hub(vec![summary("a", "running")]).await;
        let out = message(execute(&AgentCommand::Create(create_args("Bad Name")), &addr).await);
        assert!(out.contains("agent name"), "{out}");
        assert!(mock.lock().unwrap().requests.is_empty());
    }

    #[tokio::test]
    async fn create_maps_conflict_and_missing_source_to_server_message() {
        let (addr, _) = mock_hub(vec![summary("a", "running")]).await;
        let taken = message(execute(&AgentCommand::Create(create_args("taken")), &addr).await);
        assert_eq!(taken, "An agent named 'taken' already exists.");

        let mut args = create_args("fresh");
        args.models_from = Some("ghost".to_string());
        let missing = message(execute(&AgentCommand::Create(args), &addr).await);
        assert_eq!(missing, "No agent named 'ghost'.");
    }

    #[tokio::test]
    async fn delete_prints_checkpoint_id_and_restore_pointer() {
        let (addr, mock) = mock_hub(vec![]).await;
        let out = execute(
            &AgentCommand::Delete {
                name: "scout".into(),
            },
            &addr,
        )
        .await
        .unwrap();
        assert!(out.contains("Deleted agent 'scout'"));
        assert!(out.contains("Checkpoint id: abc123"));
        assert!(out.contains("restore"));
        let requests = mock.lock().unwrap().requests.clone();
        assert_eq!(requests[0].0, "DELETE");
        assert_eq!(requests[0].1, "/api/hub/agents/scout");
    }

    #[tokio::test]
    async fn delete_without_checkpoint_and_unknown_agent() {
        let (addr, _) = mock_hub(vec![]).await;
        let empty = execute(
            &AgentCommand::Delete {
                name: "empty".into(),
            },
            &addr,
        )
        .await
        .unwrap();
        assert!(empty.contains("no checkpoint"));

        let unknown = message(
            execute(
                &AgentCommand::Delete {
                    name: "ghost".into(),
                },
                &addr,
            )
            .await,
        );
        assert_eq!(unknown, "No agent named 'ghost'.");
    }

    #[tokio::test]
    async fn start_stop_restart_use_matching_routes() {
        let (addr, mock) = mock_hub(vec![]).await;
        let started = execute(
            &AgentCommand::Start {
                name: "scout".into(),
            },
            &addr,
        )
        .await
        .unwrap();
        assert_eq!(started, "Agent 'scout' is running.");
        let stopped = execute(
            &AgentCommand::Stop {
                name: "scout".into(),
            },
            &addr,
        )
        .await
        .unwrap();
        assert_eq!(stopped, "Agent 'scout' is stopped.");
        execute(
            &AgentCommand::Restart {
                name: "scout".into(),
            },
            &addr,
        )
        .await
        .unwrap();

        let paths: Vec<String> = mock
            .lock()
            .unwrap()
            .requests
            .iter()
            .map(|(m, p, _)| format!("{m} {p}"))
            .collect();
        assert_eq!(
            paths,
            [
                "POST /api/hub/agents/scout/start",
                "POST /api/hub/agents/scout/stop",
                "POST /api/hub/agents/scout/restart",
            ]
        );
    }

    #[tokio::test]
    async fn start_of_failed_agent_shows_last_error() {
        let (addr, _) = mock_hub(vec![]).await;
        let out = execute(
            &AgentCommand::Start {
                name: "broken".into(),
            },
            &addr,
        )
        .await
        .unwrap();
        assert!(out.contains("is failed"));
        assert!(out.contains("Last error: bad config"));
    }

    #[tokio::test]
    async fn lifecycle_on_unknown_agent_reports_server_message() {
        let (addr, _) = mock_hub(vec![]).await;
        let out = message(
            execute(
                &AgentCommand::Stop {
                    name: "ghost".into(),
                },
                &addr,
            )
            .await,
        );
        assert_eq!(out, "No agent named 'ghost'.");
    }

    #[tokio::test]
    async fn autostart_patches_the_flag() {
        let (addr, mock) = mock_hub(vec![]).await;
        let on = execute(
            &AgentCommand::Autostart {
                name: "scout".into(),
                setting: AutostartSetting::On,
            },
            &addr,
        )
        .await
        .unwrap();
        assert_eq!(on, "Agent 'scout' will start when Residuum starts.");
        let off = execute(
            &AgentCommand::Autostart {
                name: "scout".into(),
                setting: AutostartSetting::Off,
            },
            &addr,
        )
        .await
        .unwrap();
        assert_eq!(off, "Agent 'scout' will not start when Residuum starts.");

        let requests = mock.lock().unwrap().requests.clone();
        assert_eq!(requests[0].0, "PATCH");
        assert_eq!(requests[0].2, json!({ "autostart": true }));
        assert_eq!(requests[1].2, json!({ "autostart": false }));
    }

    #[tokio::test]
    async fn bad_request_maps_to_server_message() {
        let app = Router::new().route("/api/hub/agents/{name}/{action}", post(bad_request));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let out = message(
            execute(
                &AgentCommand::Start {
                    name: "scout".into(),
                },
                &addr,
            )
            .await,
        );
        assert_eq!(out, "Invalid agent name.");
    }
}
