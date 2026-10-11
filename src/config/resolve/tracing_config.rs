//! Tracing and log-level settings.

use crate::util::FatalError;

use super::super::constants::DEFAULT_FEEDBACK_ENDPOINT;
use super::super::deserialize::TracingConfigFile;
use super::super::types::{LogLevel, OtelEndpoint, TracingConfig};

/// Resolve tracing configuration from TOML section.
///
/// # Errors
/// Returns `FatalError::Config` if the log level string is invalid.
pub(super) fn resolve_tracing_config(
    section: Option<&TracingConfigFile>,
) -> Result<TracingConfig, FatalError> {
    let Some(section) = section else {
        return Ok(TracingConfig::default());
    };

    let log_level = section
        .log_level
        .as_deref()
        .map(str::parse::<LogLevel>)
        .transpose()
        .map_err(FatalError::Config)?
        .unwrap_or_default();

    let otel_endpoints = section
        .otel_endpoints
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|ep| OtelEndpoint {
            url: ep.url.clone(),
            name: ep.name.clone(),
            headers: ep.headers.clone().unwrap_or_default(),
        })
        .collect();

    Ok(TracingConfig {
        log_level,
        auto_error_reporting: section.auto_error_reporting.unwrap_or(false),
        sanitize_content: section.sanitize_content.unwrap_or(true),
        otel_endpoints,
        feedback_endpoint: section
            .feedback_endpoint
            .clone()
            .unwrap_or_else(|| DEFAULT_FEEDBACK_ENDPOINT.to_string()),
    })
}
