//! Tracing subcommand: manage trace export and observability.

use std::collections::HashMap;

use residuum::util::FatalError;
use serde::Deserialize;

#[derive(clap::Subcommand)]
pub(super) enum TracingCommand {
    /// Show current tracing configuration and streaming state
    Status,
    /// Manage OTEL endpoints
    Otel {
        #[command(subcommand)]
        command: OtelCommand,
    },
    /// Export buffered traces to configured OTEL endpoints
    Dump {
        /// Export to a specific endpoint URL instead of all configured endpoints
        #[arg(long)]
        endpoint: Option<String>,
    },
    /// Control live trace streaming
    Stream {
        #[command(subcommand)]
        command: StreamCommand,
    },
    /// Toggle content sanitization for trace exports
    Sanitize(SanitizeArgs),
    /// Toggle automatic error reporting
    ErrorReporting(ErrorReportingArgs),
}

#[derive(clap::Subcommand)]
pub(super) enum OtelCommand {
    /// Add an OTEL endpoint
    Add(OtelAddArgs),
    /// Remove an OTEL endpoint
    Remove(OtelRemoveArgs),
    /// List configured OTEL endpoints
    List,
    /// Test connectivity to an OTEL endpoint
    Test(OtelTestArgs),
}

#[derive(clap::Args)]
pub(super) struct OtelAddArgs {
    /// OTLP HTTP endpoint URL
    pub url: String,
    /// Human-readable name for this endpoint
    #[arg(long)]
    pub name: Option<String>,
    /// Additional HTTP headers (KEY=VALUE format, repeatable)
    #[arg(long = "header", value_name = "KEY=VALUE")]
    pub headers: Vec<String>,
}

#[derive(clap::Args)]
pub(super) struct OtelRemoveArgs {
    /// URL of the endpoint to remove
    pub url: String,
}

#[derive(clap::Args)]
pub(super) struct OtelTestArgs {
    /// URL of the endpoint to test (tests all if omitted)
    pub url: Option<String>,
}

#[derive(clap::Subcommand)]
pub(super) enum StreamCommand {
    /// Start streaming traces to OTEL endpoints
    Start,
    /// Stop streaming traces
    Stop,
}

#[derive(clap::Args)]
pub(super) struct SanitizeArgs {
    /// Enable or disable sanitization
    #[arg(value_parser = parse_on_off)]
    pub state: bool,
}

#[derive(clap::Args)]
pub(super) struct ErrorReportingArgs {
    /// Enable or disable automatic error reporting
    #[arg(value_parser = parse_on_off)]
    pub state: bool,
}

/// Parse "on"/"off" strings into booleans.
fn parse_on_off(s: &str) -> Result<bool, String> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("expected 'on' or 'off', got '{other}'")),
    }
}

/// Run a tracing subcommand by sending HTTP requests to the running daemon.
pub(super) async fn run_tracing_command(
    command: &TracingCommand,
    gateway_addr: &str,
) -> Result<(), FatalError> {
    match command {
        TracingCommand::Status => cmd_status(gateway_addr).await,
        TracingCommand::Otel { command } => cmd_otel(command, gateway_addr).await,
        TracingCommand::Dump { endpoint } => cmd_dump(gateway_addr, endpoint.as_deref()).await,
        TracingCommand::Stream { command } => cmd_stream(command, gateway_addr).await,
        TracingCommand::Sanitize(args) => cmd_sanitize(gateway_addr, args.state).await,
        TracingCommand::ErrorReporting(args) => cmd_error_reporting(gateway_addr, args.state).await,
    }
}

// ── Helper ───────────────────────────────────────────────────────────

fn make_client() -> Result<reqwest::Client, FatalError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FatalError::Gateway(format!("failed to build HTTP client: {e}")))
}

async fn api_get(gateway_addr: &str, path: &str) -> Result<reqwest::Response, FatalError> {
    let client = make_client()?;
    let url = format!("http://{gateway_addr}{path}");
    client.get(&url).send().await.map_err(|e| {
        FatalError::Gateway(format!(
            "failed to reach gateway at {gateway_addr}: {e}\nhint: is the gateway running? try 'residuum serve'"
        ))
    })
}

async fn api_post(
    gateway_addr: &str,
    path: &str,
    body: &impl serde::Serialize,
) -> Result<reqwest::Response, FatalError> {
    let client = make_client()?;
    let url = format!("http://{gateway_addr}{path}");
    client.post(&url).json(body).send().await.map_err(|e| {
        FatalError::Gateway(format!(
            "failed to reach gateway at {gateway_addr}: {e}\nhint: is the gateway running? try 'residuum serve'"
        ))
    })
}

async fn api_post_empty(gateway_addr: &str, path: &str) -> Result<reqwest::Response, FatalError> {
    let client = make_client()?;
    let url = format!("http://{gateway_addr}{path}");
    client.post(&url).send().await.map_err(|e| {
        FatalError::Gateway(format!(
            "failed to reach gateway at {gateway_addr}: {e}\nhint: is the gateway running? try 'residuum serve'"
        ))
    })
}

async fn api_delete(
    gateway_addr: &str,
    path: &str,
    body: &impl serde::Serialize,
) -> Result<reqwest::Response, FatalError> {
    let client = make_client()?;
    let url = format!("http://{gateway_addr}{path}");
    client.delete(&url).json(body).send().await.map_err(|e| {
        FatalError::Gateway(format!(
            "failed to reach gateway at {gateway_addr}: {e}\nhint: is the gateway running? try 'residuum serve'"
        ))
    })
}

fn check_response(resp: &reqwest::Response, context: &str) -> Result<(), FatalError> {
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(FatalError::Gateway(format!(
            "{context}: server returned {}",
            resp.status()
        )))
    }
}

// ── Subcommand implementations ───────────────────────────────────────

/// Response type matching `TracingStatus` from the service.
#[derive(Deserialize)]
struct TracingStatusResponse {
    log_level: String,
    auto_error_reporting: bool,
    sanitize_content: bool,
    otel_endpoints: Vec<OtelEndpointInfo>,
    streaming: bool,
    buffer_size: usize,
}

#[derive(Deserialize)]
struct OtelEndpointInfo {
    url: String,
    name: Option<String>,
}

async fn cmd_status(gateway_addr: &str) -> Result<(), FatalError> {
    let resp = api_get(gateway_addr, "/api/tracing/status").await?;
    check_response(&resp, "tracing status")?;
    let status: TracingStatusResponse = resp.json().await.map_err(|e| {
        FatalError::Gateway(format!("failed to parse tracing status response: {e}"))
    })?;

    println!("Tracing status:");
    println!("  log level:         {}", status.log_level);
    println!(
        "  error reporting:   {}",
        if status.auto_error_reporting {
            "on"
        } else {
            "off"
        }
    );
    println!(
        "  sanitize content:  {}",
        if status.sanitize_content { "on" } else { "off" }
    );
    println!(
        "  streaming:         {}",
        if status.streaming {
            "active"
        } else {
            "stopped"
        }
    );
    println!("  buffer size:       {} spans", status.buffer_size);
    println!("  otel endpoints:    {}", status.otel_endpoints.len());
    for ep in &status.otel_endpoints {
        let label = ep.name.as_deref().unwrap_or("(unnamed)");
        println!("    - {label}: {}", ep.url);
    }
    Ok(())
}

async fn cmd_otel(command: &OtelCommand, gateway_addr: &str) -> Result<(), FatalError> {
    match command {
        OtelCommand::Add(args) => {
            let mut headers = HashMap::new();
            for h in &args.headers {
                let (key, value) = h.split_once('=').ok_or_else(|| {
                    FatalError::Gateway(format!("invalid header format '{h}': expected KEY=VALUE"))
                })?;
                headers.insert(key.to_string(), value.to_string());
            }
            let body = serde_json::json!({
                "url": args.url,
                "name": args.name,
                "headers": headers,
            });
            let resp = api_post(gateway_addr, "/api/tracing/otel/endpoints", &body).await?;
            check_response(&resp, "add OTEL endpoint")?;
            println!("added endpoint: {}", args.url);
        }
        OtelCommand::Remove(args) => {
            let body = serde_json::json!({ "url": args.url });
            let resp = api_delete(gateway_addr, "/api/tracing/otel/endpoints", &body).await?;
            check_response(&resp, "remove OTEL endpoint")?;
            println!("removed endpoint: {}", args.url);
        }
        OtelCommand::List => {
            let resp = api_get(gateway_addr, "/api/tracing/otel/endpoints").await?;
            check_response(&resp, "list OTEL endpoints")?;
            let endpoints: Vec<OtelEndpointInfo> = resp.json().await.map_err(|e| {
                FatalError::Gateway(format!("failed to parse endpoints response: {e}"))
            })?;
            if endpoints.is_empty() {
                println!("no OTEL endpoints configured");
            } else {
                for ep in &endpoints {
                    let label = ep.name.as_deref().unwrap_or("(unnamed)");
                    println!("{label}: {}", ep.url);
                }
            }
        }
        OtelCommand::Test(args) => {
            let url = args.url.as_deref().unwrap_or("all");
            let body = serde_json::json!({ "url": url });
            let resp = api_post(gateway_addr, "/api/tracing/otel/test", &body).await?;
            check_response(&resp, "test OTEL connectivity")?;
            println!("connectivity test passed: {url}");
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct DumpResult {
    spans_sent: usize,
}

async fn cmd_dump(gateway_addr: &str, _endpoint: Option<&str>) -> Result<(), FatalError> {
    let resp = api_post_empty(gateway_addr, "/api/tracing/dump").await?;
    check_response(&resp, "trace dump")?;
    let result: DumpResult = resp
        .json()
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to parse dump response: {e}")))?;
    println!("exported {} spans", result.spans_sent);
    Ok(())
}

async fn cmd_stream(command: &StreamCommand, gateway_addr: &str) -> Result<(), FatalError> {
    match command {
        StreamCommand::Start => {
            let resp = api_post_empty(gateway_addr, "/api/tracing/stream/start").await?;
            check_response(&resp, "start streaming")?;
            println!("trace streaming started");
        }
        StreamCommand::Stop => {
            let resp = api_post_empty(gateway_addr, "/api/tracing/stream/stop").await?;
            check_response(&resp, "stop streaming")?;
            println!("trace streaming stopped");
        }
    }
    Ok(())
}

async fn cmd_sanitize(gateway_addr: &str, enabled: bool) -> Result<(), FatalError> {
    let body = serde_json::json!({ "enabled": enabled });
    let resp = api_post(gateway_addr, "/api/tracing/sanitize", &body).await?;
    check_response(&resp, "set sanitization")?;
    println!(
        "content sanitization: {}",
        if enabled { "on" } else { "off" }
    );
    Ok(())
}

async fn cmd_error_reporting(gateway_addr: &str, enabled: bool) -> Result<(), FatalError> {
    let body = serde_json::json!({ "enabled": enabled });
    let resp = api_post(gateway_addr, "/api/tracing/error-reporting", &body).await?;
    check_response(&resp, "set error reporting")?;
    println!(
        "auto error reporting: {}",
        if enabled { "on" } else { "off" }
    );
    Ok(())
}
