//! Helpers the hub's tests share: agents on disk that talk to a mock model
//! server, and a capture of the log events a test causes.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing_subscriber::layer::SubscriberExt as _;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mount a model reply of `text` on `server`, answering after `delay`.
pub(crate) async fn mount_reply(server: &MockServer, text: &str, delay: Duration) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(delay)
                .set_body_json(json!({
                    "choices": [{ "message": { "role": "assistant", "content": text } }]
                })),
        )
        .mount(server)
        .await;
}

/// Write a bootstrapped agent directory `root/<name>` whose main model is
/// the mock server at `model_url`.
pub(crate) fn write_agent(root: &Path, name: &str, model_url: &str) {
    let config_dir = root.join(name).join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "").unwrap();
    std::fs::write(
        config_dir.join("providers.toml"),
        format!(
            "[providers]\nmock = {{ type = \"openai\", api_key = \"test-key\", url = \"{model_url}\" }}\n\n[models]\nmain = \"mock/test-model\"\n"
        ),
    )
    .unwrap();
}

pub(crate) use crate::util::test_ports::reserve_port;

/// One log event: its level and every field rendered as `name=value`.
#[derive(Debug, Clone)]
pub(crate) struct LoggedEvent {
    pub(crate) level: tracing::Level,
    pub(crate) text: String,
}

#[derive(Clone, Default)]
pub(crate) struct EventLog {
    pub(crate) events: Arc<std::sync::Mutex<Vec<LoggedEvent>>>,
}

#[derive(Default)]
struct FieldText(Vec<String>);

impl tracing::field::Visit for FieldText {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push(format!("{}={value:?}", field.name()));
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for EventLog {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields = FieldText::default();
        event.record(&mut fields);
        self.events.lock().unwrap().push(LoggedEvent {
            level: *event.metadata().level(),
            text: fields.0.join(" "),
        });
    }
}

impl EventLog {
    /// Route this thread's log events here until the guard drops.
    pub(crate) fn capture(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()))
    }

    pub(crate) fn matching(&self, needle: &str) -> Vec<LoggedEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.text.contains(needle))
            .cloned()
            .collect()
    }
}
