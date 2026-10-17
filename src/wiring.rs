//! How the process puts the hub together with the gateway: the hub runs the
//! agents, and the gateway's launcher starts each one's runtime. The root
//! also embeds the web app, the only crate that needs `web/dist/` to build.

use std::path::Path;
use std::sync::Arc;

use crate::gateway::GatewayLauncher;
use crate::hub::web::WebAsset;
use crate::hub::{HubExit, HubInputs};
use crate::util::FatalError;

mod embedded {
    //! Module boundary isolates `rust-embed` derive from clippy `same_name_method`.
    #![expect(
        clippy::same_name_method,
        reason = "rust-embed derive generates get/iter methods that shadow trait methods"
    )]

    use rust_embed::Embed;

    /// Embedded web assets from `web/dist/`. In debug builds, files are served
    /// from disk (hot-reload); in release builds, they are compiled into the
    /// binary.
    #[derive(Embed)]
    #[folder = "web/dist/"]
    pub(super) struct WebAssets;
}

use embedded::WebAssets;

/// The embedded web app's file at `path` under `web/dist/`, in the form the
/// hub serves it.
#[must_use]
fn web_asset(path: &str) -> Option<WebAsset> {
    WebAssets::get(path).map(|file| WebAsset {
        data: file.data,
        sha256: file.metadata.sha256_hash(),
    })
}

/// Run the hub on the residuum root `root` until it is told to stop, with
/// this binary's build, the gateway's launcher, the embedded web app, the
/// readiness marker and the global log filter.
///
/// # Errors
///
/// Returns `FatalError` when the hub can't start or a hub-level service
/// fails; see [`crate::hub::run_hub`].
pub async fn run_hub(root: &Path) -> Result<HubExit, FatalError> {
    let inputs = HubInputs {
        root: root.to_path_buf(),
        build: crate::BUILD_INFO,
        launcher: Arc::new(GatewayLauncher::new()),
        assets: web_asset,
        on_ready: Box::new(crate::daemon::write_ready_file),
        log_filter: crate::logging::global_filter_handle(),
    };
    // Box::pin reduces stack frame size — this future is large
    Box::pin(crate::hub::run_hub(inputs)).await
}

#[cfg(test)]
mod tests {
    use super::{WebAssets, web_asset};
    use crate::hub::test_support::{FIXTURE_HASHED_CSS, FIXTURE_HASHED_JS, FIXTURE_PLAIN_FILES};

    #[test]
    fn web_assets_contains_index_html() {
        assert!(
            web_asset("index.html").is_some(),
            "index.html should be embedded"
        );
    }

    /// The asset tests model the real build's layout, so this checks that the
    /// build holds what they model: each plain file, a content-hashed script
    /// and stylesheet under `assets/`, the service worker's version line and
    /// the app shell's mount point.
    #[test]
    fn the_build_holds_every_plain_file_the_fixture_models() {
        for path in FIXTURE_PLAIN_FILES {
            assert!(
                web_asset(path).is_some(),
                "the web build should hold {path}, which the asset tests model"
            );
        }
        for suffix in [".js", ".css"] {
            assert!(
                WebAssets::iter().any(|path| path.starts_with("assets/") && path.ends_with(suffix)),
                "the web build should hold a content-hashed {suffix} under assets/, as the fixture models ({FIXTURE_HASHED_JS}, {FIXTURE_HASHED_CSS})"
            );
        }
        let worker = web_asset("sw.js").expect("the web build should hold sw.js");
        assert!(
            worker.data.starts_with(b"/* residuum-sw "),
            "the build names the worker's version on its first line"
        );
        let shell = web_asset("index.html").expect("the web build should hold index.html");
        assert!(
            String::from_utf8_lossy(&shell.data).contains("id=\"app\""),
            "index.html should mount the app at #app"
        );
    }
}
