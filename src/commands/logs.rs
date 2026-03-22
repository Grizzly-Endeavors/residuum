//! Logs subcommand: display and tail CLI log files.

use residuum::util::FatalError;

/// Display CLI log files.
///
/// Finds the most recent log file in `~/.residuum/logs/` and prints its
/// contents. With `--watch`, polls for new lines every 500ms.
pub(super) async fn run_logs_command(
    watch: bool,
    agent_name: Option<&str>,
) -> Result<(), FatalError> {
    let log_dir = residuum::agent_registry::paths::resolve_log_dir(agent_name)?;

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
    entries.sort_by_key(|e| {
        match e.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(path = %e.path().display(), error = %err, "failed to read log file metadata for sorting");
                std::time::SystemTime::UNIX_EPOCH
            }
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
    print!("{content}");

    if watch {
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
                    // No new data yet
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Ok(_) => {
                    print!("{line_buf}");
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
