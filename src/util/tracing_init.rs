//! Tracing initialization for all runtime modes.
//!
//! Provides three modes:
//! - `init_default_tracing`: stderr-only (most CLI subcommands)
//! - `init_cli_tracing`: stderr + file (connect client)
//! - `init_daemon_tracing`: file + optional stderr (daemon/foreground serve)

/// Initialize tracing with stderr-only output (default for serve/logs/setup).
pub fn init_default_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
}

/// Initialize tracing with both stderr and a daily rolling file appender.
///
/// Log files are written to `~/.residuum/logs/cli.YYYY-MM-DD.log`.
#[expect(
    clippy::print_stderr,
    reason = "pre-tracing startup warnings — tracing is not yet initialized"
)]
pub fn init_cli_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr);

    let log_dir = dirs::home_dir().map_or_else(
        || std::path::PathBuf::from("logs"),
        |h| h.join(".residuum").join("logs"),
    );

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .filename_prefix("cli")
        .filename_suffix("log")
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(30)
        .build(&log_dir)
        .or_else(|e| {
            eprintln!("warning: failed to create log file appender: {e}");
            eprintln!(
                "warning: logs will be written to {} instead — 'residuum logs' will not find them",
                std::env::temp_dir().display()
            );
            tracing_appender::rolling::RollingFileAppender::builder()
                .filename_prefix("cli")
                .filename_suffix("log")
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .build(std::env::temp_dir())
                .map_err(|e2| {
                    eprintln!("warning: fallback log appender also failed: {e2}; log file output disabled — 'residuum logs' will not find them");
                })
        })
        .ok();

    let file_layer = file_appender.map(|appender| {
        tracing_subscriber::fmt::layer()
            .json()
            .with_target(true)
            .with_span_list(true)
            .with_ansi(false)
            .with_writer(appender)
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();
}

/// Debug logging modes for the `--debug` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DebugMode {
    /// `--debug` (no value): residuum crates at debug, deps at warn
    #[value(name = "default")]
    Default,
    /// `--debug=all`: everything at debug
    #[value(name = "all")]
    All,
    /// `--debug=trace`: residuum crates at trace, deps at warn
    #[value(name = "trace")]
    Trace,
}

impl DebugMode {
    /// Parse a `--debug[=mode]` value into a `DebugMode`.
    ///
    /// Returns `None` for unrecognized modes (caller should report the error).
    #[must_use]
    pub fn from_flag_value(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some("") => Some(Self::Default),
            Some("all") => Some(Self::All),
            Some("trace") => Some(Self::Trace),
            Some(_) => None,
        }
    }

    /// The `EnvFilter` directive string for this mode.
    #[must_use]
    pub fn filter_str(self) -> &'static str {
        match self {
            Self::Default => "residuum=debug,warn",
            Self::All => "debug",
            Self::Trace => "residuum=trace,warn",
        }
    }
}

#[expect(
    clippy::panic,
    reason = "deliberate termination when no log appender can be created"
)]
#[expect(
    clippy::print_stderr,
    reason = "pre-tracing startup warnings — tracing is not yet initialized"
)]
fn fatal_no_log_appender(msg: &str) -> ! {
    eprintln!("{msg}");
    panic!("{msg}")
}

/// Initialize tracing with file-only output for daemonized operation.
///
/// Logs are written to `<log_dir>/serve.YYYY-MM-DD.log` (or `serve-<name>`)
/// with daily rotation and 30-day retention. When `debug_mode` is `Some`,
/// the filter is overridden accordingly and stderr output is added so debug
/// output appears in the terminal.
///
/// When `agent_name` is `Some`, logs go to the agent-specific log directory
/// and the file prefix includes the agent name for identification.
#[expect(
    clippy::print_stderr,
    reason = "pre-tracing startup warnings — tracing is not yet initialized"
)]
pub fn init_daemon_tracing(debug_mode: Option<DebugMode>, agent_name: Option<&str>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let default_filter = debug_mode.map_or("info", DebugMode::filter_str);
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));

    let log_dir = crate::agent_registry::paths::resolve_log_dir(agent_name).unwrap_or_else(|_| {
        eprintln!("warning: could not determine log directory; logs will be written to ./logs");
        std::path::PathBuf::from("logs")
    });

    let log_prefix = match agent_name {
        Some(name) => format!("serve-{name}"),
        None => "serve".to_string(),
    };

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .filename_prefix(&log_prefix)
        .filename_suffix("log")
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(30)
        .build(&log_dir)
        .unwrap_or_else(|e| {
            eprintln!(
                "warning: failed to create log file appender at {}: {e}; falling back to {}",
                log_dir.display(),
                std::env::temp_dir().display()
            );
            tracing_appender::rolling::RollingFileAppender::builder()
                .filename_prefix(&log_prefix)
                .filename_suffix("log")
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .max_log_files(30)
                .build(std::env::temp_dir())
                .unwrap_or_else(|e2| {
                    fatal_no_log_appender(&format!(
                        "fatal: could not create log appender in temp dir: {e2}"
                    ))
                })
        });

    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_target(true)
        .with_span_list(true)
        .with_ansi(false)
        .with_writer(file_appender);

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr);

    // Span buffer layer — always-on, zero-cost when nothing reads from it
    let (span_buffer_layer, span_buffer_handle) = crate::util::telemetry::SpanBufferLayer::new(
        &crate::util::telemetry::SpanBufferConfig::default(),
    );

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(debug_mode.map(|_| stderr_layer))
        .with(span_buffer_layer)
        .init();

    // Store handle globally for bug reports / live export
    if crate::util::telemetry::set_global_span_buffer(span_buffer_handle).is_err() {
        // Already set — shouldn't happen, but not fatal
        tracing::warn!("span buffer handle was already initialized");
    }

    tracing::info!(
        dir = %log_dir.display(),
        prefix = %log_prefix,
        "logging initialized (daily rotation, 30-day retention)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_mode_from_flag_value_none_is_default() {
        assert!(matches!(
            DebugMode::from_flag_value(None),
            Some(DebugMode::Default)
        ));
    }

    #[test]
    fn debug_mode_from_flag_value_empty_is_default() {
        assert!(matches!(
            DebugMode::from_flag_value(Some("")),
            Some(DebugMode::Default)
        ));
    }

    #[test]
    fn debug_mode_from_flag_value_all() {
        assert!(matches!(
            DebugMode::from_flag_value(Some("all")),
            Some(DebugMode::All)
        ));
    }

    #[test]
    fn debug_mode_from_flag_value_trace() {
        assert!(matches!(
            DebugMode::from_flag_value(Some("trace")),
            Some(DebugMode::Trace)
        ));
    }

    #[test]
    fn debug_mode_from_flag_value_unknown_is_none() {
        assert!(DebugMode::from_flag_value(Some("bogus")).is_none());
    }

    #[test]
    fn debug_mode_filter_strings() {
        assert_eq!(DebugMode::Default.filter_str(), "residuum=debug,warn");
        assert_eq!(DebugMode::All.filter_str(), "debug");
        assert_eq!(DebugMode::Trace.filter_str(), "residuum=trace,warn");
    }
}
