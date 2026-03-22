//! Tracing initialization for the binary entrypoint.
//!
//! Provides two modes: stderr-only for most subcommands, and
//! stderr + file for the connect client.

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
            crate::daemon::write_crash_note(&format!(
                "warning: failed to create log file appender: {e}"
            ));
            crate::daemon::write_crash_note(&format!(
                "warning: logs will be written to {} instead — 'residuum logs' will not find them",
                std::env::temp_dir().display()
            ));
            tracing_appender::rolling::RollingFileAppender::builder()
                .filename_prefix("cli")
                .filename_suffix("log")
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .build(std::env::temp_dir())
                .map_err(|e2| {
                    crate::daemon::write_crash_note(&format!(
                        "warning: fallback log appender also failed: {e2}"
                    ));
                    crate::daemon::write_crash_note(
                        "warning: log file output disabled — 'residuum logs' will not find them",
                    );
                })
        })
        .ok();

    let file_layer = file_appender.map(|appender| {
        tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_ansi(false)
            .with_writer(appender)
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();
}
