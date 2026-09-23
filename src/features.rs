//! The feature ids Residuum advertises to workbench artifacts and API clients.
//!
//! One list, defined once: `GET /api/status` and the workbench SDK's
//! `residuum.features` both read it, so an artifact can detect what this
//! version of Residuum supports without guessing from its version number. A
//! phase that ships a capability adds its id here.

/// Feature ids this build supports. Empty until a later phase ships a
/// capability with a feature id.
pub(crate) const FEATURES: &[&str] = &[];
