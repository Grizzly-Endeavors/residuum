//! Tracing initialization for all runtime modes.
//!
//! Provides three modes:
//! - `init_default_tracing`: stderr-only (most CLI subcommands)
//! - `init_cli_tracing`: stderr + file (connect client)
//! - `init_daemon_tracing`: file + optional stderr (daemon/foreground serve)

use std::sync::OnceLock;

use crate::config::LogLevel;

// ── Runtime filter reload ────────────────────────────────────────────

/// Trait for runtime log level switching.
///
/// Type-erases the `tracing_subscriber::reload::Handle` so it can be stored
/// globally without naming the full subscriber stack type.
pub trait FilterReload: Send + Sync {
    /// Replace the active `EnvFilter` with one for the given log level.
    ///
    /// # Errors
    /// Returns an error if the filter cannot be applied.
    fn set_filter(&self, level: LogLevel) -> Result<(), String>;
}

/// Process-global filter reload handle, set once during daemon tracing init.
static FILTER_HANDLE: OnceLock<Box<dyn FilterReload>> = OnceLock::new();

/// Access the global filter reload handle.
///
/// Returns `None` if daemon tracing was not initialized (e.g. in CLI or test modes).
#[must_use]
pub fn global_filter_handle() -> Option<&'static dyn FilterReload> {
    FILTER_HANDLE.get().map(AsRef::as_ref)
}

/// Set the global filter reload handle. Called once during daemon tracing init.
///
/// Returns `Err` if the handle was already set.
fn set_global_filter_handle(handle: Box<dyn FilterReload>) -> Result<(), Box<dyn FilterReload>> {
    FILTER_HANDLE.set(handle)
}

// ── Initialization modes ─────────────────────────────────────────────

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

/// Initialize tracing for daemonized operation.
///
/// Logs are written to `<log_dir>/serve.YYYY-MM-DD.log` (or `serve-<name>`)
/// with daily rotation and 30-day retention. The log level is determined by
/// the `log_level` parameter from config.toml (overridden by `RUST_LOG` if set).
///
/// When `foreground` is true, stderr output is added so logs appear in the
/// terminal. The filter level applies equally to file and stderr output.
///
/// A reload handle is stored globally so the log level can be changed at
/// runtime via config reload.
#[expect(
    clippy::print_stderr,
    reason = "pre-tracing startup warnings — tracing is not yet initialized"
)]
pub fn init_daemon_tracing(foreground: bool, agent_name: Option<&str>, log_level: LogLevel) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let default_filter = log_level.filter_str();
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));

    // Wrap in a reload layer so the filter can be changed at runtime
    let (reload_filter, reload_handle) = tracing_subscriber::reload::Layer::new(env_filter);

    // Store the reload handle globally for runtime filter changes
    let handle = ReloadFilterHandle {
        inner: reload_handle,
    };
    if set_global_filter_handle(Box::new(handle)).is_err() {
        eprintln!("warning: filter reload handle was already initialized");
    }

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
        .with(reload_filter)
        .with(file_layer)
        .with(foreground.then_some(stderr_layer))
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
        level = %log_level,
        foreground,
        "logging initialized (daily rotation, 30-day retention)"
    );
}

// ── Reload handle implementation ─────────────────────────────────────

/// Concrete `FilterReload` implementation wrapping a `tracing_subscriber::reload::Handle`.
///
/// The generic types capture the subscriber stack at the point where the reload
/// layer was inserted. We use the trait to erase these types so the handle can
/// be stored in a `OnceLock`.
struct ReloadFilterHandle<S: Send + Sync> {
    inner: tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, S>,
}

impl<S> FilterReload for ReloadFilterHandle<S>
where
    S: tracing::Subscriber + Send + Sync + 'static,
{
    fn set_filter(&self, level: LogLevel) -> Result<(), String> {
        let new_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level.filter_str()));
        self.inner
            .modify(|filter| *filter = new_filter)
            .map_err(|e| format!("failed to update log filter: {e}"))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use crate::config::LogLevel;

    #[test]
    fn log_level_filter_strings() {
        assert_eq!(LogLevel::Info.filter_str(), "info");
        assert_eq!(LogLevel::Debug.filter_str(), "residuum=debug,warn");
        assert_eq!(LogLevel::Trace.filter_str(), "residuum=trace,warn");
    }

    #[test]
    fn log_level_from_str() {
        assert_eq!("info".parse::<LogLevel>().unwrap(), LogLevel::Info);
        assert_eq!("debug".parse::<LogLevel>().unwrap(), LogLevel::Debug);
        assert_eq!("trace".parse::<LogLevel>().unwrap(), LogLevel::Trace);
        assert!("bogus".parse::<LogLevel>().is_err());
    }

    #[test]
    fn log_level_display_round_trips() {
        for level in [LogLevel::Info, LogLevel::Debug, LogLevel::Trace] {
            let s = level.to_string();
            assert_eq!(s.parse::<LogLevel>().unwrap(), level);
        }
    }
}
