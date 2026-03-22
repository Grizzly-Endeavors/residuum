Analyze this module for logging hygiene issues.

For each finding, be specific — reference file paths and line numbers. Focus on actionable findings, not praise.

## What to look for

### Incorrect log levels
- `error!` used for recoverable situations (should be `warn!`).
- `warn!` or `info!` used for failures that stop an operation (should be `error!`).
- Internal state transitions or implementation details at `info!` (should be `debug!`).
- Verbose diagnostics, full payloads, or timing data at `debug!` (should be `trace!`).
- The expected levels:
  - **error**: failures that stop an operation
  - **warn**: recoverable issues, degraded behavior
  - **info**: major operations (LLM calls, chunked processing)
  - **debug**: internal details, state transitions
  - **trace**: verbose diagnostics (full payloads, timing)

### String interpolation instead of structured fields
- `info!("processing {count} chunks")` should be `info!(chunks = count, "starting chunked review")`.
- `format!()` or `format_args!()` used inside log macros when structured fields would work.
- Structured fields make logs queryable — string interpolation buries data in the message.

### Log spam patterns
- Logging on every loop iteration or timer tick at `info!` or above. These belong at `trace!` at most.
- Routine successful operations logged at `info!` or above ("connection still alive", "heartbeat ok"). Absence of errors is the signal that things work.
- Periodic health-check output above `trace!` level.

### Missing context
- Log entries that would be useless for debugging in production. Ask: "If I saw only this log line in a dashboard, could I identify what failed and where?"
- Missing structured fields for key identifiers (IDs, paths, counts, durations).
- Bare messages like `error!("failed")` or `warn!("retry")` with no qualifying information.

### Inconsistent patterns
- Mix of `tracing` and `log` macros in the same module.
- Inconsistent field naming across related log entries (e.g., `path` in one place, `file_path` in another for the same concept).
- Log messages that use different tenses, capitalization, or punctuation styles within the module.

## Output format

If the module's logging is clean, say so. "No findings" is a valid and good outcome. Don't manufacture findings.

If there are findings, organize by category (use the headings above). For each finding:
- State what the problem is
- Reference the specific file and line(s)
- Propose a concrete fix

Skip any category that has no findings.
