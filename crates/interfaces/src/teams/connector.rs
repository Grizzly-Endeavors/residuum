//! Outbound calls to the Bot Connector: token acquisition and sending activities.

use std::time::{Duration, Instant};

use serde::Deserialize;

use super::store::ConversationRef;

const CONNECTOR_SCOPE: &str = "https://api.botframework.com/.default";
/// Refresh the connector token this long before it actually expires.
const TOKEN_EXPIRY_MARGIN: Duration = Duration::from_mins(5);
/// Attempts per send when the connector throttles us (HTTP 429).
const MAX_SEND_ATTEMPTS: u32 = 3;
/// Backoff when a 429 carries no usable `Retry-After`.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(2);

/// Why an outbound call to Teams failed.
#[derive(Debug, thiserror::Error)]
pub(super) enum ConnectorError {
    #[error("could not get a bot token from Microsoft: {0}")]
    Token(String),
    #[error("request to the bot connector failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("bot connector returned {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

struct CachedToken {
    value: String,
    refresh_at: Instant,
}

/// Credentials and a cached bearer token for the Bot Connector.
pub(super) struct ConnectorClient {
    http: reqwest::Client,
    token_url: String,
    app_id: String,
    app_password: String,
    token: tokio::sync::Mutex<Option<CachedToken>>,
}

impl ConnectorClient {
    pub(super) fn new(
        http: reqwest::Client,
        tenant_id: &str,
        app_id: String,
        app_password: String,
    ) -> Self {
        Self {
            http,
            token_url: format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token"),
            app_id,
            app_password,
            token: tokio::sync::Mutex::new(None),
        }
    }

    /// A valid connector bearer token, fetched with client credentials when needed.
    pub(super) async fn bearer_token(&self) -> Result<String, ConnectorError> {
        let mut cached = self.token.lock().await;
        if let Some(token) = cached.as_ref().filter(|t| Instant::now() < t.refresh_at) {
            return Ok(token.value.clone());
        }

        let form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "client_credentials")
            .append_pair("client_id", &self.app_id)
            .append_pair("client_secret", &self.app_password)
            .append_pair("scope", CONNECTOR_SCOPE)
            .finish();
        let response = self
            .http
            .post(&self.token_url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form)
            .send()
            .await
            .map_err(|e| ConnectorError::Token(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            // Entra's error body names the problem (bad secret, wrong tenant)
            // without echoing the secret, so it is safe and useful to surface.
            let body = response.text().await.unwrap_or_default();
            return Err(ConnectorError::Token(format!("{status}: {body}")));
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|e| ConnectorError::Token(e.to_string()))?;
        let lifetime = Duration::from_secs(token.expires_in).saturating_sub(TOKEN_EXPIRY_MARGIN);
        *cached = Some(CachedToken {
            value: token.access_token.clone(),
            refresh_at: Instant::now() + lifetime,
        });
        tracing::debug!(
            expires_in_secs = token.expires_in,
            "teams connector token refreshed"
        );
        Ok(token.access_token)
    }

    /// Post an activity into a conversation.
    pub(super) async fn send_activity(
        &self,
        target: &ConversationRef,
        activity: &serde_json::Value,
    ) -> Result<(), ConnectorError> {
        let url = activities_url(&target.service_url, &target.conversation_id);
        let mut attempt = 1;
        loop {
            let token = self.bearer_token().await?;
            let response = self
                .http
                .post(&url)
                .bearer_auth(&token)
                .json(activity)
                .send()
                .await?;
            let status = response.status();
            if status.is_success() {
                return Ok(());
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_SEND_ATTEMPTS {
                let wait = retry_after(&response).unwrap_or(DEFAULT_RETRY_AFTER);
                if attempt == 1 {
                    tracing::warn!(
                        conversation = %target.conversation_id,
                        wait_ms = wait.as_millis(),
                        max_attempts = MAX_SEND_ATTEMPTS,
                        "teams connector throttled the bot, retrying"
                    );
                }
                tokio::time::sleep(wait).await;
                attempt += 1;
                continue;
            }
            if status == reqwest::StatusCode::UNAUTHORIZED {
                // A revoked or rotated secret invalidates the cached token early.
                self.token.lock().await.take();
            }
            let body = response.text().await.unwrap_or_default();
            return Err(ConnectorError::Status { status, body });
        }
    }

    /// Download an attachment the connector hosts (inline images in chats),
    /// which requires the bot's bearer token.
    pub(super) async fn download(&self, url: &str) -> Result<Vec<u8>, ConnectorError> {
        let token = self.bearer_token().await?;
        let response = self.http.get(url).bearer_auth(&token).send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ConnectorError::Status { status, body });
        }
        Ok(response.bytes().await?.to_vec())
    }
}

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// `{serviceUrl}/v3/conversations/{id}/activities`, with the conversation ID
/// percent-encoded (channel thread IDs contain `;` and `=`).
fn activities_url(service_url: &str, conversation_id: &str) -> String {
    let encoded: String =
        url::form_urlencoded::byte_serialize(conversation_id.as_bytes()).collect();
    format!(
        "{}/v3/conversations/{encoded}/activities",
        service_url.trim_end_matches('/')
    )
}

/// A plain markdown message activity.
pub(super) fn message_activity(text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "textFormat": "markdown",
        "text": text,
    })
}

/// A typing indicator activity.
pub(super) fn typing_activity() -> serde_json::Value {
    serde_json::json!({ "type": "typing" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activities_url_encodes_thread_ids() {
        assert_eq!(
            activities_url(
                "https://smba.trafficmanager.net/amer/",
                "19:abc@thread.tacv2;messageid=17"
            ),
            "https://smba.trafficmanager.net/amer/v3/conversations/19%3Aabc%40thread.tacv2%3Bmessageid%3D17/activities"
        );
    }
}
