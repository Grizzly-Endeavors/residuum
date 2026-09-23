//! The feature ids Residuum advertises to workbench artifacts and API clients.
//!
//! One list, defined once: `GET /api/status` and the workbench SDK's
//! `residuum.features` both read it, so an artifact can detect what this
//! version of Residuum supports without guessing from its version number.
//! Every capability an artifact can detect has its id here.

/// Feature ids this build supports.
pub(crate) const FEATURES: &[&str] = &[
    "workspace-tree",
    "workspace-read-batch",
    "inbox-add",
    "memory-search",
    "artifact-state",
    "workspace-raw",
    "workspace-conditional-write",
    "workspace-file-ops",
    "artifact-sessions",
];
