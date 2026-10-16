//! Helpers the hub's tests share: agents on disk that talk to a mock model
//! server, API states whose checkpoint engine watches the directories the
//! handlers write to, and a fixture web app for the asset routes.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::checkpoints::CheckpointEngine;
use crate::hub::web::{ConfigApiState, HubApiState, WebAsset, WorkspaceScope};

/// Mount a model reply of `text` on `server`. A test that needs the reply to
/// wait puts a gate in front of the server instead.
pub(crate) async fn mount_reply(server: &MockServer, text: &str) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": text } }]
        })))
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

/// The content-hashed build output the fixture web app serves for JavaScript,
/// named as the real build names it.
pub(crate) const FIXTURE_HASHED_JS: &str = "assets/fixture-app.js";

/// The content-hashed build output the fixture web app serves for styles.
pub(crate) const FIXTURE_HASHED_CSS: &str = "assets/fixture-app.css";

/// The fixture's files that are not content-hashed. The real build holds each
/// of them too, which the root's `wiring` tests check.
pub(crate) const FIXTURE_PLAIN_FILES: [&str; 6] = [
    "index.html",
    "mcp-catalog.json",
    "manifest.webmanifest",
    "sw.js",
    "favicon.svg",
    "icons/icon-192.png",
];

/// A web app for the asset tests: the files the handler tests request, with
/// the content types, cache classes and sizes those tests assert on. Each
/// file has its own digest, so its `ETag` differs from the others.
pub(crate) fn fixture_assets(asset_path: &str) -> Option<WebAsset> {
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
            &crate::config::paths::TeamPaths::new(hub_dir.clone().join("team")),
            config_dir.clone(),
            hub_dir.clone(),
            &hub_dir.join("checkpoints"),
            None,
        )
        .unwrap(),
    );
    (hub_dir, config_dir, workspace_dir, checkpoints)
}

/// An agent-scope [`ConfigApiState`] whose checkpoints watch `root`.
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

/// A [`HubApiState`] whose checkpoints watch `root`, serving [`fixture_assets`].
pub(crate) fn watching_hub_state(root: &Path) -> HubApiState {
    let (hub_dir, _config_dir, _workspace_dir, checkpoints) = watching_engine(root);
    let mut state = HubApiState::for_test(&hub_dir, fixture_assets);
    state.checkpoints = checkpoints;
    state
}
