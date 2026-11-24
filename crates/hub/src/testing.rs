use std::borrow::Cow;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use residuum_checkpoints::CheckpointEngine;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::web::WebAsset;
#[cfg(test)]
use crate::web::{ConfigApiState, HubApiState, WorkspaceScope};

pub async fn mount_reply(server: &MockServer, text: &str) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": text } }]
        })))
        .mount(server)
        .await;
}

#[expect(
    clippy::unwrap_used,
    reason = "test-support helper: a fixture that cannot be written fails the test that asked for it"
)]
pub fn write_agent(root: &Path, name: &str, model_url: &str) {
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

pub const FIXTURE_HASHED_JS: &str = "assets/fixture-app.js";

pub const FIXTURE_HASHED_CSS: &str = "assets/fixture-app.css";

pub const FIXTURE_PLAIN_FILES: [&str; 6] = [
    "index.html",
    "mcp-catalog.json",
    "manifest.webmanifest",
    "sw.js",
    "favicon.svg",
    "icons/icon-192.png",
];

#[must_use]
pub fn fixture_assets(asset_path: &str) -> Option<WebAsset> {
    let (data, digest): (Cow<'static, [u8]>, u8) = match asset_path {
        "index.html" => (
            Cow::Borrowed(b"<!doctype html><title>fixture</title><div id=\"app\"></div>"),
            1,
        ),
        "mcp-catalog.json" => (Cow::Borrowed(br#"{"servers":[]}"#), 2),
        "manifest.webmanifest" => (Cow::Borrowed(br#"{"name":"fixture"}"#), 3),
        "sw.js" => (
            Cow::Borrowed(
                b"/* residuum-sw fixture */\nself.addEventListener('fetch', () => {});\n",
            ),
            4,
        ),
        "favicon.svg" => (
            Cow::Borrowed(br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#),
            5,
        ),
        "icons/icon-192.png" => (Cow::Borrowed(b"\x89PNG\r\n\x1a\n"), 6),
        FIXTURE_HASHED_JS => (
            Cow::Owned(
                "function fixture(a) { return a + 1; }\n"
                    .repeat(200)
                    .into_bytes(),
            ),
            7,
        ),
        FIXTURE_HASHED_CSS => (
            Cow::Owned(
                ".fixture { color: rebeccapurple; }\n"
                    .repeat(200)
                    .into_bytes(),
            ),
            8,
        ),
        _ => return None,
    };
    Some(WebAsset {
        data,
        sha256: [digest; 32],
    })
}

/// The API states whose checkpoint engine watches the same config and
/// workspace directories the handlers write to. `test_engine` deliberately
/// does not, so a test that asserts on the returned checkpoint id needs these.
#[cfg(test)]
fn watching_engine(root: &Path) -> (PathBuf, PathBuf, PathBuf, Arc<CheckpointEngine>) {
    let config_dir = root.join("config");
    let workspace_dir = root.join("workspace");
    let hub_dir = root.join("hub");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::create_dir_all(&hub_dir).unwrap();
    let checkpoints = Arc::new(
        CheckpointEngine::new(
            "test-agent",
            workspace_dir.clone(),
            &residuum_config::paths::TeamPaths::new(hub_dir.clone().join("team")),
            config_dir.clone(),
            hub_dir.clone(),
            &hub_dir.join("checkpoints"),
            None,
        )
        .unwrap(),
    );
    (hub_dir, config_dir, workspace_dir, checkpoints)
}

#[cfg(test)]
pub(crate) fn watching_state(root: &Path) -> ConfigApiState {
    let (hub_dir, config_dir, workspace_dir, checkpoints) = watching_engine(root);
    ConfigApiState {
        team: None,
        hub_dir,
        config_dir,
        agent_name: "test-agent".to_string(),
        workspace_dir,
        memory_dir: None,
        reload_tx: None,
        checkpoints,
        scope: WorkspaceScope::Agent,
    }
}

#[cfg(test)]
pub(crate) fn watching_hub_state(root: &Path) -> HubApiState {
    let (hub_dir, _config_dir, _workspace_dir, checkpoints) = watching_engine(root);
    let mut state = HubApiState::for_test(&hub_dir, fixture_assets);
    state.checkpoints = checkpoints;
    state
}
