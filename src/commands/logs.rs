//! Logs subcommand: display and tail structured log files.
//!
//! Reads NDJSON log files produced by `tracing-subscriber`'s JSON formatter,
//! applies optional module/level filters, and renders human-readable output.

use residuum::logging::format::{
    LogEntry, LogLevel, format_entry, format_entry_colored, meets_level, module_matches, parse_line,
};
use residuum::util::FatalError;
use std::io::Write;

#[derive(clap::Args)]
pub(super) struct LogsArgs {
    /// Tail the log file, polling for new lines
    #[arg(long, short)]
    pub watch: bool,
    /// Filter by module (e.g., `agent`, `mcp`, `gateway`, `residuum::mcp::client`)
    #[arg(long, short)]
    pub module: Option<String>,
    /// Filter by minimum log level (trace, debug, info, warn, error)
    #[arg(long, short)]
    pub level: Option<String>,
    /// Show only lines from one agent (matches the `agent` field on the line or its spans)
    #[arg(long, short)]
    pub agent: Option<String>,
    /// Output raw JSON instead of formatted text
    #[arg(long)]
    pub json: bool,
}

/// Resolved filter criteria, computed once from CLI args.
struct LogFilter {
    module: Option<String>,
    min_level: Option<LogLevel>,
    agent: Option<String>,
    raw_json: bool,
    color: bool,
}

/// The line printed when a non-watch run shows no entries, naming only the filters given.
fn no_match_notice(args: &LogsArgs) -> Option<String> {
    let mut given = Vec::new();
    if let Some(module) = &args.module {
        given.push(format!("--module {module}"));
    }
    if let Some(level) = &args.level {
        given.push(format!("--level {level}"));
    }
    if let Some(agent) = &args.agent {
        given.push(format!("--agent {agent}"));
    }
    (!given.is_empty()).then(|| format!("No log entries matched ({}).", given.join(", ")))
}

impl LogFilter {
    fn from_args(args: &LogsArgs) -> Result<Self, FatalError> {
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
            module: args.module.clone(),
            min_level,
            agent: args.agent.clone(),
            raw_json: args.json,
            color,
        })
    }

    /// Format and print a log line, applying filters. Returns true if the line was printed.
    fn process_line(&self, line: &str) -> bool {
        match self.render_line(line) {
            Some(text) => {
                println!("{text}");
                true
            }
            None => false,
        }
    }

    /// Apply the filters to one log line and return the text to print, if any.
    fn render_line(&self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let Some(entry) = parse_line(trimmed) else {
            // A line that isn't a JSON log entry (a partial write, or text from another source) is printed raw,
            // unless filtering by agent: an unparsed line can't be attributed.
            return self.agent.is_none().then(|| trimmed.to_string());
        };

        if let Some(ref module) = self.module
            && !module_matches(module, &entry.target)
        {
            return None;
        }

        if let Some(min) = self.min_level
            && !meets_level(&entry.level, min)
        {
            return None;
        }

        if let Some(ref agent) = self.agent
            && !entry_belongs_to_agent(&entry, agent)
        {
            return None;
        }

        Some(if self.raw_json {
            trimmed.to_string()
        } else if self.color {
            format_entry_colored(&entry)
        } else {
            format_entry(&entry)
        })
    }
}

/// Whether a log entry carries `agent = <name>` on the event or on any
/// enclosing span.
fn entry_belongs_to_agent(entry: &LogEntry, agent: &str) -> bool {
    let matches =
        |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_str) == Some(agent);
    matches(entry.fields.get("agent"))
        || entry
            .spans
            .iter()
            .any(|span| matches(span.fields.get("agent")))
}

/// Writes the entries of one log file that pass the filter to `out`. A non-watch
/// run that prints no entries writes the no-match notice to `err` instead: it is
/// command output for the person running the command, not a diagnostic, so it
/// is not routed through tracing.
fn print_entries(
    filter: &LogFilter,
    args: &LogsArgs,
    content: &str,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<()> {
    let mut printed = 0_usize;
    for line in content.lines() {
        if let Some(text) = filter.render_line(line) {
            writeln!(out, "{text}")?;
            printed += 1;
        }
    }
    if !args.watch
        && printed == 0
        && let Some(notice) = no_match_notice(args)
    {
        writeln!(err, "{notice}")?;
    }
    Ok(())
}

/// Display and optionally tail structured log files.
///
/// Finds the most recent log file in the log directory, parses JSON lines,
/// applies filters, and renders human-readable output. With `--watch`, polls
/// for new lines every 500ms.
#[tracing::instrument(skip_all)]
pub(super) async fn run_logs_command(args: &LogsArgs) -> Result<(), FatalError> {
    let filter = LogFilter::from_args(args)?;
    let log_dir = residuum::config::HubPaths::new(residuum::config::default_hub_dir()?).logs_dir();

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
    {
        let mut out = std::io::stdout().lock();
        let mut err = std::io::stderr().lock();
        print_entries(&filter, args, &content, &mut out, &mut err)
            .map_err(|e| FatalError::Config(format!("failed to write log output: {e}")))?;
    }

    if args.watch {
        use tokio::io::{AsyncBufReadExt, AsyncSeekExt};

        let file = tokio::fs::File::open(&latest)
            .await
            .map_err(|e| FatalError::Config(format!("failed to open log file for watch: {e}")))?;
        let mut reader = tokio::io::BufReader::new(file);

        // Seek to current end
        reader
            .seek(std::io::SeekFrom::End(0))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(agent: Option<&str>, level: Option<LogLevel>) -> LogFilter {
        LogFilter {
            module: None,
            min_level: level,
            agent: agent.map(str::to_string),
            raw_json: false,
            color: false,
        }
    }

    fn line(level: &str, fields: &str, spans: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-09-29T10:00:00Z","level":"{level}","target":"residuum::x","fields":{fields},"spans":{spans}}}"#
        )
    }

    #[test]
    fn agent_filter_matches_event_field() {
        let f = filter(Some("scout"), None);
        let hit = line("INFO", r#"{"message":"hi","agent":"scout"}"#, "[]");
        let miss = line("INFO", r#"{"message":"hi","agent":"other"}"#, "[]");
        assert!(f.render_line(&hit).is_some());
        assert!(f.render_line(&miss).is_none());
    }

    #[test]
    fn agent_filter_matches_span_field() {
        let f = filter(Some("scout"), None);
        let hit = line(
            "INFO",
            r#"{"message":"hi"}"#,
            r#"[{"name":"agent","agent":"scout"}]"#,
        );
        assert!(f.render_line(&hit).is_some());
    }

    #[test]
    fn agent_filter_drops_untagged_and_unparsed_lines() {
        let f = filter(Some("scout"), None);
        assert!(
            f.render_line(&line("INFO", r#"{"message":"hi"}"#, "[]"))
                .is_none()
        );
        assert!(f.render_line("not json").is_none());
    }

    #[test]
    fn no_agent_filter_keeps_everything() {
        let f = filter(None, None);
        assert!(
            f.render_line(&line("INFO", r#"{"message":"hi"}"#, "[]"))
                .is_some()
        );
        assert_eq!(f.render_line("not json").as_deref(), Some("not json"));
    }

    #[test]
    fn module_matching_nothing_gives_notice_and_no_entries() {
        let f = LogFilter {
            module: Some("nosuchmodule".to_string()),
            ..filter(None, None)
        };
        let args = LogsArgs {
            watch: false,
            module: Some("nosuchmodule".to_string()),
            level: None,
            agent: None,
            json: false,
        };
        let content = [
            line("INFO", r#"{"message":"hi"}"#, "[]"),
            line("WARN", r#"{"message":"there"}"#, "[]"),
        ]
        .join("\n");
        let (mut out, mut err): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
        print_entries(&f, &args, &content, &mut out, &mut err)
            .expect("writing to a Vec cannot fail");
        assert_eq!(String::from_utf8(out).expect("entries are UTF-8"), "");
        assert_eq!(
            String::from_utf8(err).expect("notice is UTF-8"),
            "No log entries matched (--module nosuchmodule).\n"
        );

        let watching = LogsArgs {
            watch: true,
            ..args
        };
        let (mut watch_out, mut watch_err): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
        print_entries(&f, &watching, &content, &mut watch_out, &mut watch_err)
            .expect("writing to a Vec cannot fail");
        assert_eq!(String::from_utf8(watch_err).expect("UTF-8"), "");
    }

    #[test]
    fn agent_filter_composes_with_level() {
        let f = filter(Some("scout"), Some(LogLevel::Warn));
        let info = line("INFO", r#"{"agent":"scout"}"#, "[]");
        let warn = line("WARN", r#"{"agent":"scout"}"#, "[]");
        let other_warn = line("WARN", r#"{"agent":"b"}"#, "[]");
        assert!(f.render_line(&info).is_none());
        assert!(f.render_line(&warn).is_some());
        assert!(f.render_line(&other_warn).is_none());
    }
}
