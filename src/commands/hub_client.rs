//! HTTP client for CLI commands that talk to the running hub.
//!
//! Turns transport failures and the server's `{ "error": "…" }` bodies into
//! plain-language messages, so no command shows raw HTTP or socket errors.

use reqwest::Method;
use serde::de::DeserializeOwned;

use residuum::util::FatalError;

const NOT_RUNNING_MESSAGE: &str = "Residuum isn't running. Start it with `residuum serve`.";

/// Client for the hub's `/api/hub/...` routes on the local gateway address.
pub(super) struct HubClient {
    addr: String,
    http: reqwest::Client,
}

impl HubClient {
    /// Build a client for the gateway listening at `addr` (`host:port`).
    pub(super) fn new(addr: &str) -> Result<Self, FatalError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| {
                tracing::error!(error = %e, "failed to build HTTP client");
                user_error("Couldn't set up the connection to Residuum.".to_string())
            })?;
        Ok(Self {
            addr: addr.to_string(),
            http,
        })
    }

    /// Send a request that answers with no body when it succeeds.
    ///
    /// Failures are reported as [`Self::send`] reports them.
    pub(super) async fn send_no_content(
        &self,
        method: Method,
        path: &str,
    ) -> Result<(), FatalError> {
        self.execute(method, path, None).await.map(drop)
    }

    /// Send a request with a JSON body that answers with no body when it
    /// succeeds.
    pub(super) async fn send_no_content_with(
        &self,
        method: Method,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(), FatalError> {
        self.execute(method, path, Some(body)).await.map(drop)
    }

    /// Send a request and decode the JSON response body.
    ///
    /// Non-success statuses become an error carrying the server's message
    /// (`400`, `404`, `409`) or a generic failure line naming the status.
    pub(super) async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, FatalError> {
        let response = self.execute(method, path, body).await?;
        response.json::<T>().await.map_err(|e| {
            tracing::error!(error = %e, path, "failed to decode hub response");
            user_error(
                "Residuum sent a response this version of the CLI doesn't understand. Update the CLI and the daemon to the same version."
                    .to_string(),
            )
        })
    }

    async fn execute(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response, FatalError> {
        let url = format!("http://{}{path}", self.addr);
        let mut request = self.http.request(method.clone(), &url);
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().await.map_err(|e| {
            tracing::warn!(error = %e, addr = %self.addr, path, %method, "hub request failed");
            if e.is_timeout() {
                user_error(
                    "Residuum didn't respond in time. Check `residuum logs` for details."
                        .to_string(),
                )
            } else {
                user_error(NOT_RUNNING_MESSAGE.to_string())
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let server_message = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));
            tracing::warn!(
                status = status.as_u16(),
                path,
                %method,
                message = server_message.as_deref().unwrap_or(""),
                "hub request rejected"
            );
            return Err(user_error(match (status.as_u16(), server_message) {
                (400 | 404 | 409, Some(message)) => capitalize(&message),
                (_, Some(message)) => format!(
                    "Residuum couldn't complete the request ({}): {message}",
                    status.as_u16()
                ),
                (code, None) => format!(
                    "Residuum couldn't complete the request (status {code}). Check `residuum logs` for details."
                ),
            }));
        }
        Ok(response)
    }
}

fn user_error(message: String) -> FatalError {
    FatalError::Other(anyhow::anyhow!(message))
}

fn capitalize(message: &str) -> String {
    let mut chars = message.chars();
    chars.next().map_or_else(String::new, |first| {
        let mut out: String = first.to_uppercase().collect();
        out.push_str(chars.as_str());
        if !out.ends_with(['.', '!', '?']) {
            out.push('.');
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalize_adds_initial_and_period() {
        assert_eq!(capitalize("no agent named 'x'"), "No agent named 'x'.");
        assert_eq!(capitalize("Already done."), "Already done.");
        assert_eq!(capitalize(""), "");
    }
}
