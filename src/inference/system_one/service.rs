//! The hub's one System 1 service: the configured client, its health, and
//! the check that clears an outage once the service answers again.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::client::{SystemOneClient, SystemOneEndpoint};
use super::error::SystemOneError;
use super::health::SystemOneStatus;
use super::types::{Question, SystemOneResponse};
use crate::config::SystemOneConfig;
use crate::inference::retry::RetryConfig;
use crate::inference::{HttpClientConfig, SharedHttpClient};

/// How often a down service is checked while an outage lasts, so the outage
/// clears even when nothing else is asking for decisions.
const RECOVERY_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Request timeout. A local Ollama loading a model for the first request can
/// take well over the inference default.
const REQUEST_TIMEOUT_SECS: u64 = 120;

/// The endpoint a [`SystemOneConfig`] describes.
#[must_use]
pub fn endpoint_from_config(cfg: &SystemOneConfig) -> SystemOneEndpoint {
    SystemOneEndpoint {
        provider: cfg.display_name(),
        base_url: cfg.url.clone(),
        model: cfg.model.clone(),
        api_key: cfg.api_key.clone(),
        keep_alive: cfg.keep_alive.clone(),
    }
}

/// Build a client for `cfg` with its own connection pool, for one-off calls
/// such as testing a configuration that hasn't been saved.
///
/// # Errors
/// Returns a message when the HTTP client can't be built.
pub fn client_for_config(cfg: &SystemOneConfig) -> Result<SystemOneClient, String> {
    let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(REQUEST_TIMEOUT_SECS))
        .map_err(|e| format!("failed to build the HTTP client: {e}"))?;
    Ok(SystemOneClient::new(http, endpoint_from_config(cfg)))
}

/// The configured System 1 client shared by every agent, with its health.
pub struct SystemOneService {
    client: RwLock<Option<SystemOneClient>>,
    status: watch::Sender<SystemOneStatus>,
    recovery: Mutex<Option<JoinHandle<()>>>,
    recovery_interval: Duration,
}

impl SystemOneService {
    /// A service for `cfg`; `None` leaves it unconfigured.
    #[must_use]
    pub fn new(cfg: Option<&SystemOneConfig>) -> Arc<Self> {
        Self::with_recovery_interval(cfg, RECOVERY_CHECK_INTERVAL)
    }

    fn with_recovery_interval(
        cfg: Option<&SystemOneConfig>,
        recovery_interval: Duration,
    ) -> Arc<Self> {
        let (client, status) = Self::build(cfg);
        Arc::new(Self {
            client: RwLock::new(client),
            status: watch::channel(status).0,
            recovery: Mutex::new(None),
            recovery_interval,
        })
    }

    fn build(cfg: Option<&SystemOneConfig>) -> (Option<SystemOneClient>, SystemOneStatus) {
        let Some(cfg) = cfg else {
            return (None, SystemOneStatus::new(None, None));
        };
        let status = SystemOneStatus::new(Some(cfg.display_name()), Some(cfg.model.clone()));
        match client_for_config(cfg) {
            Ok(client) => (Some(client), status),
            Err(e) => {
                tracing::error!(error = %e, "failed to build the system one client");
                (None, status)
            }
        }
    }

    /// Replace the configuration. Any outage clears: the next call says
    /// whether the new one works.
    pub fn reconfigure(&self, cfg: Option<&SystemOneConfig>) {
        let (client, status) = Self::build(cfg);
        *self.client.write().unwrap_or_else(PoisonError::into_inner) = client;
        self.stop_recovery_check();
        self.status.send_replace(status);
        tracing::info!(
            configured = cfg.is_some(),
            "system one configuration updated"
        );
    }

    /// Watch the status; the receiver sees the current value first.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<SystemOneStatus> {
        self.status.subscribe()
    }

    /// The current status.
    #[must_use]
    pub fn status(&self) -> SystemOneStatus {
        self.status.borrow().clone()
    }

    /// Ask `questions` about `state`, recording the outcome in the status.
    ///
    /// # Errors
    /// [`SystemOneError::NotConfigured`] when no provider is set, otherwise
    /// whatever the call failed with.
    pub async fn evaluate(
        self: &Arc<Self>,
        state: &Value,
        questions: &BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, SystemOneError> {
        let client = self
            .client
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let result = match client {
            Some(client) => client.evaluate(state, questions).await,
            None => Err(SystemOneError::NotConfigured),
        };
        match &result {
            Ok(_) => self.record_success(),
            Err(e) if e.affects_health() => self.record_failure(e),
            Err(e) => {
                tracing::warn!(error = %e, "system one rejected one request");
            }
        }
        result
    }

    fn record_success(&self) {
        let ended = self
            .status
            .send_if_modified(SystemOneStatus::record_success);
        if ended {
            tracing::info!("system one service is answering again");
            self.stop_recovery_check();
        }
    }

    fn record_failure(self: &Arc<Self>, error: &SystemOneError) {
        let was_down = self.status.borrow().outage.is_some();
        let changed = self.status.send_if_modified(|s| s.record_failure(error));
        if !was_down {
            tracing::warn!(
                error = %error,
                kind = ?error.outage_kind(),
                "system one service is unavailable; decisions that need it are skipped until it answers"
            );
        } else if changed {
            tracing::warn!(error = %error, "system one outage changed");
        }
        if !matches!(error, SystemOneError::NotConfigured) {
            self.start_recovery_check();
        }
    }

    fn start_recovery_check(self: &Arc<Self>) {
        let mut slot = self.recovery.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let weak = Arc::downgrade(self);
        let interval = self.recovery_interval;
        // The check serves the whole hub, so it logs under its own span
        // rather than that of whichever agent's call noticed the outage.
        let task = tracing::Instrument::instrument(
            recovery_check(weak, interval),
            tracing::info_span!("system_one_recovery"),
        );
        *slot = Some(crate::util::spawn_in_span(task));
    }

    fn stop_recovery_check(&self) {
        if let Some(task) = self
            .recovery
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

impl Drop for SystemOneService {
    fn drop(&mut self) {
        self.stop_recovery_check();
    }
}

/// Every `interval` while an outage lasts, send the smallest possible
/// request; the first success clears the outage through the normal path.
async fn recovery_check(service: Weak<SystemOneService>, interval: Duration) {
    let state = Value::String("Hello".to_string());
    let questions = BTreeMap::from([(
        "greeting".to_string(),
        Question::noul("Is this text a greeting?"),
    )]);
    loop {
        tokio::time::sleep(interval).await;
        let Some(service) = service.upgrade() else {
            return;
        };
        if service.status.borrow().outage.is_none() {
            return;
        }
        let client = service
            .client
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(client) = client else {
            return;
        };
        match client
            .with_retry(RetryConfig::no_retry())
            .evaluate(&state, &questions)
            .await
        {
            Ok(_) => {
                service.record_success();
                return;
            }
            Err(e) => {
                tracing::trace!(error = %e, "system one recovery check failed");
                if e.affects_health() {
                    service.status.send_if_modified(|s| s.record_failure(&e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::SystemOneProvider;
    use crate::inference::system_one::SystemOneOutageKind;

    fn config(url: &str) -> SystemOneConfig {
        SystemOneConfig {
            provider: SystemOneProvider::Ollama,
            url: url.to_string(),
            model: "nimble".to_string(),
            api_key: None,
            keep_alive: None,
        }
    }

    fn questions() -> BTreeMap<String, Question> {
        BTreeMap::from([("q".to_string(), Question::noul("Is it?"))])
    }

    fn ok_body() -> serde_json::Value {
        json!({ "model": "nimble", "answers": { "q": { "type": "noul", "noul": 0.1 } } })
    }

    #[tokio::test]
    async fn unconfigured_service_reports_not_configured() {
        let service = SystemOneService::new(None);
        let err = service
            .evaluate(&json!("x"), &questions())
            .await
            .unwrap_err();
        assert_eq!(err, SystemOneError::NotConfigured);
        let status = service.status();
        assert!(!status.configured);
        assert_eq!(
            status.outage.map(|o| o.kind),
            Some(SystemOneOutageKind::NotConfigured),
            "an unconfigured service that was asked shows why"
        );
    }

    #[tokio::test]
    async fn outage_publishes_once_and_recovery_check_clears_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(4)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;

        let service = SystemOneService::with_recovery_interval(
            Some(&config(&server.uri())),
            Duration::from_millis(20),
        );
        let no_retry = service
            .client
            .read()
            .unwrap()
            .clone()
            .map(|c| c.with_retry(RetryConfig::no_retry()));
        *service.client.write().unwrap() = no_retry;
        let mut rx = service.subscribe();
        rx.borrow_and_update();

        assert!(service.evaluate(&json!("x"), &questions()).await.is_err());
        assert!(rx.has_changed().unwrap(), "the outage is published");
        rx.borrow_and_update();
        assert!(service.evaluate(&json!("x"), &questions()).await.is_err());
        assert!(
            !rx.has_changed().unwrap(),
            "a repeat of the same outage is not published again"
        );

        tokio::time::timeout(Duration::from_secs(5), async {
            while service.status().outage.is_some() {
                rx.changed().await.unwrap();
            }
        })
        .await
        .expect("the recovery check should clear the outage");
    }

    #[tokio::test]
    async fn reconfigure_clears_an_outage() {
        let service = SystemOneService::new(None);
        service.evaluate(&json!("x"), &questions()).await.ok();
        assert!(service.status().outage.is_some());
        service.reconfigure(Some(&config("http://127.0.0.1:9")));
        let status = service.status();
        assert!(status.configured);
        assert!(status.outage.is_none(), "a new config starts clean");
        assert_eq!(status.model.as_deref(), Some("nimble"));
    }
}
