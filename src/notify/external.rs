//! External notification channels: ntfy and webhook.

use async_trait::async_trait;
use reqwest::Client;

use crate::bus::NotificationEvent;

use super::channels::NotificationChannel;

/// Ntfy push notification channel.
pub struct NtfyChannel {
    channel_name: String,
    client: Client,
    url: String,
    topic: String,
    priority: String,
}

impl NtfyChannel {
    /// Create a new ntfy channel.
    #[must_use]
    pub fn new(
        channel_name: String,
        client: Client,
        url: String,
        topic: String,
        priority: Option<String>,
    ) -> Self {
        Self {
            channel_name,
            client,
            url,
            topic,
            priority: priority.unwrap_or_else(|| "default".to_string()),
        }
    }
}

#[async_trait]
impl NotificationChannel for NtfyChannel {
    fn name(&self) -> &str {
        &self.channel_name
    }

    fn channel_kind(&self) -> &'static str {
        "ntfy"
    }

    async fn deliver(&self, notification: &NotificationEvent) -> anyhow::Result<()> {
        let endpoint = format!("{}/{}", self.url.trim_end_matches('/'), self.topic);
        let title = format!("[{}] {}", notification.source.as_str(), notification.title);

        tracing::debug!(
            channel = %self.channel_name,
            endpoint = %endpoint,
            title = %notification.title,
            "delivering ntfy notification"
        );

        let resp = self
            .client
            .post(&endpoint)
            .header("Title", title)
            .header("Priority", &self.priority)
            .body(notification.content.clone())
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "ntfy returned status {} for channel '{}'",
                resp.status(),
                self.channel_name
            );
        }

        Ok(())
    }
}

/// Webhook notification channel.
pub struct WebhookChannel {
    channel_name: String,
    client: Client,
    url: String,
    method: String,
    headers: Vec<(String, String)>,
}

impl WebhookChannel {
    /// Create a new webhook channel.
    #[must_use]
    pub fn new(
        channel_name: String,
        client: Client,
        url: String,
        method: Option<String>,
        headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            channel_name,
            client,
            url,
            method: method.unwrap_or_else(|| "POST".to_string()).to_uppercase(),
            headers,
        }
    }
}

#[async_trait]
impl NotificationChannel for WebhookChannel {
    fn name(&self) -> &str {
        &self.channel_name
    }

    fn channel_kind(&self) -> &'static str {
        "webhook"
    }

    async fn deliver(&self, notification: &NotificationEvent) -> anyhow::Result<()> {
        tracing::debug!(
            channel = %self.channel_name,
            endpoint = %self.url,
            title = %notification.title,
            "delivering webhook notification"
        );

        let payload = serde_json::json!({
            "title": notification.title,
            "content": notification.content,
            "timestamp": notification.timestamp.to_string(),
            "source": notification.source.as_str(),
        });

        let mut builder = match self.method.as_str() {
            "POST" => self.client.post(&self.url),
            "PUT" => self.client.put(&self.url),
            m => anyhow::bail!(
                "unsupported webhook method '{}' for channel '{}'",
                m,
                self.channel_name
            ),
        };

        for (key, val) in &self.headers {
            builder = builder.header(key.as_str(), val.as_str());
        }

        let resp = builder.json(&payload).send().await?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "webhook returned status {} for channel '{}'",
                resp.status(),
                self.channel_name
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntfy_channel_name() {
        let channel = NtfyChannel::new(
            "my_ntfy".to_string(),
            Client::new(),
            "https://ntfy.sh".to_string(),
            "test".to_string(),
            None,
        );
        assert_eq!(channel.name(), "my_ntfy");
    }

    #[test]
    fn ntfy_default_priority() {
        let channel = NtfyChannel::new(
            "ntfy".to_string(),
            Client::new(),
            "https://ntfy.sh".to_string(),
            "test".to_string(),
            None,
        );
        assert_eq!(channel.priority, "default");
    }

    #[test]
    fn ntfy_custom_priority() {
        let channel = NtfyChannel::new(
            "ntfy".to_string(),
            Client::new(),
            "https://ntfy.sh".to_string(),
            "test".to_string(),
            Some("high".to_string()),
        );
        assert_eq!(channel.priority, "high");
    }

    #[test]
    fn webhook_channel_name() {
        let channel = WebhookChannel::new(
            "ops_webhook".to_string(),
            Client::new(),
            "https://hooks.example.com".to_string(),
            None,
            Vec::new(),
        );
        assert_eq!(channel.name(), "ops_webhook");
    }

    #[test]
    fn webhook_default_method() {
        let channel = WebhookChannel::new(
            "wh".to_string(),
            Client::new(),
            "https://hooks.example.com".to_string(),
            None,
            Vec::new(),
        );
        assert_eq!(channel.method, "POST");
    }

    #[test]
    fn webhook_custom_method() {
        let channel = WebhookChannel::new(
            "wh".to_string(),
            Client::new(),
            "https://hooks.example.com".to_string(),
            Some("PUT".to_string()),
            Vec::new(),
        );
        assert_eq!(channel.method, "PUT");
    }
}
