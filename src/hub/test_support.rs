//! Helpers the hub's tests share: agents on disk that talk to a mock model
//! server.

use std::path::Path;
use std::time::Duration;

use serde_json::json;
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

pub(crate) use crate::util::test_ports::free_port;
