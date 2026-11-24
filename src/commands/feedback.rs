use residuum::util::FatalError;
use serde::Deserialize;

#[derive(clap::Args)]
pub(super) struct FeedbackArgs {
    #[arg(short, long)]
    pub message: String,
    #[arg(short, long)]
    pub category: Option<String>,
}

#[derive(Deserialize)]
struct SubmissionReceipt {
    public_id: String,
}

pub(super) async fn run_feedback_command(
    args: &FeedbackArgs,
    gateway_addr: &str,
) -> Result<(), FatalError> {
    let url = format!("http://{gateway_addr}/api/hub/tracing/feedback");
    let body = serde_json::json!({
        "message": args.message,
        "category": args.category,
    });
    let client = super::hub_client::local_hub_client()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| FatalError::Gateway(format!("failed to build HTTP client: {e}")))?;

    let resp = client.post(&url).json(&body).send().await.map_err(|e| {
        FatalError::Gateway(format!(
            "failed to reach gateway at {gateway_addr}: {e}\nhint: is the gateway running? try 'residuum serve'"
        ))
    })?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(FatalError::Gateway(format!(
            "feedback submission failed ({status}): {}",
            if body_text.is_empty() {
                "no response body".to_string()
            } else {
                body_text
            }
        )));
    }

    let receipt: SubmissionReceipt = resp
        .json()
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to parse feedback response: {e}")))?;

    println!("Feedback submitted: {}", receipt.public_id);
    Ok(())
}
