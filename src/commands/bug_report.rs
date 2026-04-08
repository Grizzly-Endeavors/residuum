//! Bug report subcommand: send a trace dump to the developer.

use residuum::util::FatalError;
use serde::Deserialize;

#[derive(clap::Args)]
pub(super) struct BugReportArgs {
    /// Description of the issue
    #[arg(short, long)]
    pub message: String,
}

#[derive(Deserialize)]
struct BugReportResult {
    spans_sent: usize,
    endpoint_results: Vec<EndpointResult>,
}

#[derive(Deserialize)]
struct EndpointResult {
    success: bool,
    error: Option<String>,
}

/// Send a bug report with trace dump to the built-in developer endpoint.
pub(super) async fn run_bug_report_command(
    args: &BugReportArgs,
    gateway_addr: &str,
) -> Result<(), FatalError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| FatalError::Gateway(format!("failed to build HTTP client: {e}")))?;

    let url = format!("http://{gateway_addr}/api/tracing/bug-report");
    let body = serde_json::json!({ "message": args.message });

    let resp = client.post(&url).json(&body).send().await.map_err(|e| {
        FatalError::Gateway(format!(
            "failed to reach gateway at {gateway_addr}: {e}\nhint: is the gateway running? try 'residuum serve'"
        ))
    })?;

    if !resp.status().is_success() {
        return Err(FatalError::Gateway(format!(
            "bug report failed: server returned {}",
            resp.status()
        )));
    }

    let result: BugReportResult = resp
        .json()
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to parse bug report response: {e}")))?;

    if result.endpoint_results.iter().all(|r| !r.success) {
        if let Some(first_error) = result
            .endpoint_results
            .first()
            .and_then(|r| r.error.as_ref())
        {
            println!("residuum: {first_error}");
        }
    } else {
        println!("bug report sent ({} spans included)", result.spans_sent);
    }

    Ok(())
}
