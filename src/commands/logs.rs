//! Logs subcommand: display and tail structured log files.
//!
//! Reads NDJSON log files produced by `tracing-subscriber`'s JSON formatter,
//! applies optional module/level filters, and renders human-readable output.

use residuum::util::FatalError;
use residuum::util::log_format::{
    LogLevel, expand_module_filter, format_entry, format_entry_colored, matches_module,
    meets_level, parse_line,
};

#[derive(clap::Args)]
pub(super) struct LogsArgs {
    /// Tail the log file, polling for new lines
    #[arg(long, short)]
    pub watch: bool,
    /// Target a named agent instance
    #[arg(long)]
    pub agent: Option<String>,
    /// Filter by module (e.g., `agent`, `mcp`, `gateway`, `residuum::mcp::client`)
    #[arg(long, short)]
    pub module: Option<String>,
    /// Filter by minimum log level (trace, debug, info, warn, error)
    #[arg(long, short)]
    pub level: Option<String>,
    /// Output raw JSON instead of formatted text
    #[arg(long)]
    pub json: bool,
}

/// Resolved filter criteria, computed once from CLI args.
struct LogFilter {
    module_prefix: Option<String>,
    min_level: Option<LogLevel>,
    raw_json: bool,
    color: bool,
}

impl LogFilter {
    fn from_args(args: &LogsArgs) -> Result<Self, FatalError> {
        let module_prefix = args.module.as_deref().map(expand_module_filter);
        let min_level = args
            .level
            .as_deref()
            .map(|l| {
                LogLevel::parse(l).ok_or_else(|| {
                    FatalError::Config(format!(
                        "invalid log level '{l}' — expected trace, debug, info, warn, or error"
                    ))
                })
            })
            .transpose()?;
        let color = std::io::IsTerminal::is_terminal(&std::io::stdout())
            && !args.json
            && std::env::var_os("NO_COLOR").is_none();
        Ok(Self {
            module_prefix,
            min_level,
            raw_json: args.json,
            color,
        })
    }

    /// Format and print a log line, applying filters. Returns true if the line was printed.
    fn process_line(&self, line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }

        let Some(entry) = parse_line(trimmed) else {
            // Non-JSON line (old format or partial write) — print raw as fallback
            println!("{trimmed}");
            return true;
        };

        if let Some(ref prefix) = self.module_prefix
            && !matches_module(&entry.target, prefix)
        {
            return false;
        }

        if let Some(min) = self.min_level
            && !meets_level(&entry.level, min)
        {
            return false;
        }

        if self.raw_json {
            println!("{trimmed}");
        } else if self.color {
            println!("{}", format_entry_colored(&entry));
        } else {
            println!("{}", format_entry(&entry));
        }
        true
    }
}

/// Display and optionally tail structured log files.
///
/// Finds the most recent log file in the log directory, parses JSON lines,
/// applies filters, and renders human-readable output. With `--watch`, polls
/// for new lines every 500ms.
pub(super) async fn run_logs_command(args: &LogsArgs) -> Result<(), FatalError> {
    let filter = LogFilter::from_args(args)?;
    let log_dir = residuum::agent_registry::paths::resolve_log_dir(args.agent.as_deref())?;

    if !log_dir.exists() {
        println!(
            "no log files found (directory does not exist: {})",
            log_dir.display()
        );
        return Ok(());
    }

    // Find the most recent log file
    let mut entries: Vec<_> = std::fs::read_dir(&log_dir)
        .map_err(|e| FatalError::Config(format!("failed to read log directory: {e}")))?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .collect();

    if entries.is_empty() {
        println!("no log files found in {}", log_dir.display());
        return Ok(());
    }

    // Sort by modification time, most recent last
    entries.sort_by_key(|e| match e.metadata().and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(err) => {
            tracing::warn!(path = %e.path().display(), error = %err, "failed to read log file metadata for sorting");
            std::time::SystemTime::UNIX_EPOCH
        }
    });

    let Some(latest_entry) = entries.last() else {
        return Ok(());
    };
    let latest = latest_entry.path();

    println!("showing: {}", latest.display());
    println!();

    let content = std::fs::read_to_string(&latest)
        .map_err(|e| FatalError::Config(format!("failed to read log file: {e}")))?;
    for line in content.lines() {
        filter.process_line(line);
    }

    if args.watch {
        use tokio::io::{AsyncBufReadExt, AsyncSeekExt};

        let file = tokio::fs::File::open(&latest)
            .await
            .map_err(|e| FatalError::Config(format!("failed to open log file for watch: {e}")))?;
        let mut reader = tokio::io::BufReader::new(file);

        // Seek to current end
        let metadata = std::fs::metadata(&latest)
            .map_err(|e| FatalError::Config(format!("failed to stat log file: {e}")))?;
        let file_len = metadata.len();
        reader
            .seek(std::io::SeekFrom::Start(file_len))
            .await
            .map_err(|e| FatalError::Config(format!("failed to seek log file: {e}")))?;

        let mut line_buf = String::new();
        loop {
            line_buf.clear();
            match reader.read_line(&mut line_buf).await {
                Ok(0) => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Ok(_) => {
                    filter.process_line(&line_buf);
                }
                Err(e) => {
                    println!("error reading log file: {e}");
                    println!(
                        "  hint: the log file may have been rotated — re-run 'residuum logs --watch' to follow the new file"
                    );
                    break;
                }
            }
        }
    }

    Ok(())
}
