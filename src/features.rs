//! The feature ids Residuum advertises to workbench artifacts and API clients.
//!
//! One list, defined once: `GET /api/status` and the workbench SDK's
//! `residuum.features` both read it, so an artifact can detect what this
//! version of Residuum supports without guessing from its version number. A
//! phase that ships a capability adds its id here.

/// Feature ids this build supports. A later phase ships more capabilities
/// and adds their ids.
pub(crate) const FEATURES: &[&str] = &["workspace-tree", "workspace-read-batch"];
