use std::path::Path;
use std::sync::Arc;

use crate::gateway::GatewayLauncher;
use crate::hub::web::WebAsset;
use crate::hub::{HubExit, HubInputs};
use crate::util::FatalError;

mod embedded {
    #![expect(
        clippy::same_name_method,
        reason = "rust-embed derive generates get/iter methods that shadow trait methods"
    )]

    use rust_embed::Embed;

    #[derive(Embed)]
    #[folder = "web/dist/"]
    pub(super) struct WebAssets;
}

use embedded::WebAssets;

#[must_use]
fn web_asset(path: &str) -> Option<WebAsset> {
    WebAssets::get(path).map(|file| WebAsset {
        data: file.data,
        sha256: file.metadata.sha256_hash(),
    })
}

pub async fn run_hub(root: &Path) -> Result<HubExit, FatalError> {
    let inputs = HubInputs {
        root: root.to_path_buf(),
        build: crate::BUILD_INFO,
        launcher: Arc::new(GatewayLauncher::new()),
        assets: web_asset,
        on_ready: Box::new(crate::daemon::write_ready_file),
        log_filter: crate::logging::global_filter_handle(),
    };
    Box::pin(crate::hub::run_hub(inputs)).await
}

#[cfg(test)]
mod tests {
    use super::{WebAssets, web_asset};
    use crate::hub::testing::{FIXTURE_HASHED_CSS, FIXTURE_HASHED_JS, FIXTURE_PLAIN_FILES};

    #[test]
    fn web_assets_contains_index_html() {
        assert!(
            web_asset("index.html").is_some(),
            "index.html should be embedded"
        );
    }

    #[test]
    fn the_build_holds_every_plain_file_the_fixture_models() {
        const REBUILD: &str = "web/dist may be stale or incomplete: rebuild it with `just web-build`, or run the tests with `just test`, which rebuilds it when needed";
        for path in FIXTURE_PLAIN_FILES {
            assert!(
                web_asset(path).is_some(),
                "the web build should hold {path}, which the asset tests model. {REBUILD}"
            );
        }
        for suffix in [".js", ".css"] {
            assert!(
                WebAssets::iter().any(|path| path.starts_with("assets/") && path.ends_with(suffix)),
                "the web build should hold a content-hashed {suffix} under assets/, as the fixture models ({FIXTURE_HASHED_JS}, {FIXTURE_HASHED_CSS}). {REBUILD}"
            );
        }
        let Some(worker) = web_asset("sw.js") else {
            panic!("the web build should hold sw.js. {REBUILD}");
        };
        assert!(
            worker.data.starts_with(b"/* residuum-sw "),
            "the build names the worker's version on its first line. {REBUILD}"
        );
        let Some(shell) = web_asset("index.html") else {
            panic!("the web build should hold index.html. {REBUILD}");
        };
        assert!(
            String::from_utf8_lossy(&shell.data).contains("id=\"app\""),
            "index.html should mount the app at #app. {REBUILD}"
        );
    }
}
