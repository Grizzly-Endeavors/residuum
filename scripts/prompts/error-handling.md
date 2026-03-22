Analyze this module for error handling violations.

For each finding, be specific — reference file paths and line numbers. Focus on actionable findings, not praise.

## What to look for

### Silent failures
- Error paths that don't produce a log entry or user-facing message. Every failure must be visible.
- `.ok()`, `let _ =`, or discarded `Result` values that swallow errors without logging.
- Match arms or `if let` patterns that silently drop the error case.

### User-facing message quality
- Error messages shown to users that expose internals (raw error types, stack traces, module paths, technical jargon).
- Messages that don't explain what went wrong and what to do next.
- Good: `"Couldn't connect to the server. Check your internet connection and try again."`
- Bad: `"TCP connection refused on port 443"`, `"IoError: connection reset"`

### Developer-facing diagnostic quality
- Error logs missing structured fields. Should be `error!(error = %e, path = %path, "failed to read config")` — not just a bare message string.
- Missing anyhow context chains. Functions that propagate errors with bare `?` where `.context("failed to ...")` would add diagnostic value.
- Error logs that lack enough information to diagnose the failure without reproducing it.

### Partial failure reporting
- Operations that process multiple items (syncing, batching, iterating) where some can fail independently.
- These must report what succeeded, what failed, and whether the user needs to act — not just "an error occurred."

### Log level correctness for errors
- Failures that stop an operation should be `error!`, not `warn!` or `info!`.
- Recoverable issues or degraded behavior should be `warn!`, not `error!`.
- Retries and transient failures logged at `error!` when `warn!` is appropriate.

### Retry noise
- Each retry attempt logged individually instead of logging once when retries start and once when they resolve or exhaust.
- Missing attempt count or backoff details on retry warnings.

## Output format

If the module's error handling is clean, say so. "No findings" is a valid and good outcome. Don't manufacture findings.

If there are findings, organize by category (use the headings above). For each finding:
- State what the problem is
- Reference the specific file and line(s)
- Propose a concrete fix

Skip any category that has no findings.
