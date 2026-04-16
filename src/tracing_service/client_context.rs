//! Builds the `client` context block attached to every bug-report and
//! feedback submission.
//!
//! This module is the only place that decides what runtime metadata the
//! agent shares with the developer endpoint. Everything excluded here —
//! chat history, memory, file contents, API keys, file paths — is
//! intentionally out of scope.

use crate::config::Config;

use super::{ClientContext, FeedbackClient};

/// Gather the full client context for a bug report.
///
/// Reads version/commit from build-time env vars, OS/arch from
/// `std::env::consts`, and the active model from the resolved config.
///
/// `active_subagents` and `config_flags` are intentionally empty in v1
/// — see follow-up issues for live-instance tracking and the curated
/// allowlist pass.
#[must_use]
pub fn gather_for_bug_report(config: &Config) -> ClientContext {
    let (model_provider, model_name) = config.main.first().map_or((None, None), |spec| {
        (
            Some(spec.model.kind.to_string()),
            Some(spec.model.model.clone()),
        )
    });

    ClientContext {
        version: env!("RESIDUUM_VERSION").to_string(),
        commit: option_env!("RESIDUUM_GIT_COMMIT").map(str::to_string),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        model_provider,
        model_name,
        active_subagents: Vec::new(),
        config_flags: std::collections::BTreeMap::new(),
    }
}

/// Gather the (version-only) client context for a feedback submission.
///
/// The feedback wire contract accepts `client.version` only; this
/// helper exists so callers don't accidentally over-attach metadata.
#[must_use]
pub fn gather_for_feedback() -> FeedbackClient {
    FeedbackClient {
        version: env!("RESIDUUM_VERSION").to_string(),
    }
}
