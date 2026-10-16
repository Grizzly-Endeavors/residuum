//! How the process puts the hub together with the gateway: the hub runs the
//! agents, and the gateway's launcher starts each one's runtime.

use std::path::Path;
use std::sync::Arc;

use crate::gateway::GatewayLauncher;
use crate::hub::{HubExit, HubInputs};
use crate::util::FatalError;

/// Run the hub on the residuum root `root` until it is told to stop, with
/// this binary's build, the gateway's launcher, the readiness marker and the
/// global log filter.
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
        on_ready: Box::new(crate::daemon::write_ready_file),
        log_filter: crate::logging::global_filter_handle(),
    };
    // Box::pin reduces stack frame size — this future is large
    Box::pin(crate::hub::run_hub(inputs)).await
}
