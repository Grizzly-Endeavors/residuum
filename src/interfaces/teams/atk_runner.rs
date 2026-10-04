//! Microsoft 365 Agents Toolkit (ATK) process runner and streaming execution layer.
//!
//! Provides bounded log capture with monotonic sequence numbers, process-group isolated
//! execution, prerequisite detection, non-interactive sign-in detection, and login URL parsing.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncBufReadExt;
use tokio_util::sync::CancellationToken;

use crate::config::paths::{agent_dir, hub_dir};
use crate::interfaces::teams::atk::{
    ATK_CLI_VERSION, derive_cloud_teams_endpoint, forward_redirect, resolve_atk_paths,
};
use crate::interfaces::teams::setup_types::{
    AtkStatus, LogLine, LogStream, SetupError, SuggestedEndpointSource, TeamsSetupPhase,
    TeamsSetupPrereqs, ToolStatus,
};
use crate::util::FatalError;
use crate::util::process::{
    ProcessTreeGuard, create_argv_command, is_process_running, kill_process_tree,
    kill_process_tree_sync,
};

/// Default capacity for bounded execution log (1000 lines).
pub const DEFAULT_LOG_CAPACITY: usize = 1000;

/// Documentation URL for the manual Teams setup walkthrough.
pub const MANUAL_GUIDE_URL: &str = "https://residuum.dev/docs/guides/teams-setup";

/// Minimum supported Node.js version constant.
pub const MIN_NODE_VERSION: &str = "12.0.0";

/// Thread-safe bounded log buffer with monotonically increasing sequence numbers.
#[derive(Debug, Clone)]
pub struct BoundedLog {
    max_lines: usize,
    next_seq: Arc<AtomicU64>,
    entries: Arc<Mutex<VecDeque<LogLine>>>,
}

impl Default for BoundedLog {
    fn default() -> Self {
        Self::new(DEFAULT_LOG_CAPACITY)
    }
}

impl BoundedLog {
    /// Create a new bounded log buffer with the given capacity.
    #[must_use]
    pub fn new(max_lines: usize) -> Self {
        Self {
            max_lines,
            next_seq: Arc::new(AtomicU64::new(1)),
            entries: Arc::new(Mutex::new(VecDeque::with_capacity(max_lines))),
        }
    }

    /// Append a new line to the log buffer, returning the recorded entry.
    #[expect(
        clippy::must_use_candidate,
        reason = "push returns LogLine for convenience but is primarily called for side-effects"
    )]
    pub fn push(&self, stream: LogStream, text: String) -> LogLine {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let at = chrono::Utc::now().to_rfc3339();
        let line = LogLine {
            seq,
            stream,
            text,
            at,
        };

        if let Ok(mut lock) = self.entries.lock() {
            if lock.len() >= self.max_lines {
                lock.pop_front();
            }
            lock.push_back(line.clone());
        }
        line
    }

    /// Retrieve all log entries with sequence number strictly greater than `log_since`.
    #[must_use]
    pub fn lines_since(&self, log_since: u64) -> Vec<LogLine> {
        let Ok(lock) = self.entries.lock() else {
            return Vec::new();
        };
        lock.iter()
            .filter(|line| line.seq > log_since)
            .cloned()
            .collect()
    }

    /// Retrieve the most recent `n` log lines.
    #[must_use]
    pub fn tail(&self, n: usize) -> Vec<LogLine> {
        let Ok(lock) = self.entries.lock() else {
            return Vec::new();
        };
        let start = lock.len().saturating_sub(n);
        lock.iter().skip(start).cloned().collect()
    }

    /// Return the sequence number of the most recently written line, or 0 if empty.
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        self.next_seq.load(Ordering::SeqCst).saturating_sub(1)
    }

    /// Return all lines formatted as plain text separated by newlines.
    #[must_use]
    pub fn full_text(&self) -> String {
        let Ok(lock) = self.entries.lock() else {
            return String::new();
        };
        lock.iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Strip ANSI escape codes from output text.
#[must_use]
pub fn strip_ansi_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_escape = false;
    for ch in input.chars() {
        if ch == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if ch.is_ascii_alphabetic() || ch == '\\' {
                in_escape = false;
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse Microsoft 365 sign-in URL and redirect port from interactive CLI output.
///
/// Looks for lines matching:
/// `Log in to your Microsoft 365 account - opening default web browser at <url>`
/// and extracts `redirect_uri` port parameter.
#[must_use]
pub fn parse_login_url_and_port(line: &str) -> Option<(String, u16)> {
    const MARKER: &str = "opening default web browser at";
    let stripped = strip_ansi_codes(line);
    let trimmed = stripped.trim();

    let url_candidate = if let Some(idx) = trimmed.find(MARKER) {
        trimmed.get(idx + MARKER.len()..)?.trim()
    } else if trimmed.starts_with("https://login.microsoftonline.com") {
        trimmed
    } else {
        return None;
    };

    let clean_url = url_candidate.trim_end_matches('#').trim();
    let parsed_url = url::Url::parse(clean_url).ok()?;

    for (k, v) in parsed_url.query_pairs() {
        if k == "redirect_uri" {
            if let Ok(redirect_parsed) = url::Url::parse(&v)
                && let Some(port) = redirect_parsed.port()
            {
                return Some((clean_url.to_string(), port));
            }
            if let Some(port) = extract_port_from_redirect_str(&v) {
                return Some((clean_url.to_string(), port));
            }
        }
    }

    if let Some(port) = extract_port_from_redirect_str(clean_url) {
        return Some((clean_url.to_string(), port));
    }

    None
}

fn extract_port_from_redirect_str(s: &str) -> Option<u16> {
    for needle in ["localhost:", "127.0.0.1:", "localhost%3A", "127.0.0.1%3A"] {
        if let Some(pos) = s.find(needle) {
            let rest = s.get(pos + needle.len()..)?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(port) = digits.parse::<u16>() {
                return Some(port);
            }
        }
    }
    None
}

/// Map npm CLI installation errors into plain-language, actionable explanations.
#[must_use]
pub fn map_npm_error(stderr_text: &str, exit_code: Option<i32>) -> String {
    let lower = stderr_text.to_ascii_lowercase();
    if lower.contains("eacces") || lower.contains("permission denied") {
        "Permission denied when installing Microsoft 365 Agents Toolkit. Check directory permissions and try again.".to_string()
    } else if lower.contains("enotfound")
        || lower.contains("econnrefused")
        || lower.contains("etimedout")
        || lower.contains("fetch failed")
        || lower.contains("network")
        || lower.contains("eai_again")
    {
        "Could not connect to the npm registry to install Microsoft 365 Agents Toolkit. Check your internet connection and try again.".to_string()
    } else if lower.contains("enoent") && lower.contains("npm") {
        "npm is not installed or not found in PATH. Please install Node.js and npm to continue."
            .to_string()
    } else {
        format!(
            "Failed to install Microsoft 365 Agents Toolkit (exit code {}). {}",
            exit_code.unwrap_or(-1),
            stderr_text.trim()
        )
    }
}

/// Map ATK app installation / sideloading errors into plain-language explanations.
#[must_use]
pub fn map_install_app_error(output_text: &str) -> String {
    let lower = output_text.to_ascii_lowercase();
    if lower.contains("sideloadingdisabled")
        || lower.contains("upload permission")
        || lower.contains("custom app upload")
        || lower.contains("admin approval")
        || lower.contains("forbidden")
    {
        "Custom app uploading is blocked by your Microsoft 365 tenant policy. Please download the app package zip and submit it to your IT administrator for approval.".to_string()
    } else {
        format!(
            "Failed to install app in Microsoft Teams: {}",
            output_text.trim()
        )
    }
}

/// Optional path overrides for injected test executables without global PATH modification.
#[derive(Debug, Clone, Default)]
pub struct AtkRunnerOverrides {
    /// Injected path for `node`.
    pub node_bin: Option<PathBuf>,
    /// Injected path for `npm`.
    pub npm_bin: Option<PathBuf>,
    /// Injected path for `atk`.
    pub atk_bin: Option<PathBuf>,
}

/// Result of a streaming process execution.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    /// Process exit code if terminated normally.
    pub exit_code: Option<i32>,
    /// Whether the process exited with code 0.
    pub success: bool,
    /// Complete captured standard output.
    pub stdout: String,
    /// Complete captured standard error.
    pub stderr: String,
}

/// Errors occurring during process execution.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("command execution timed out after {0:?}")]
    TimedOut(Duration),
    #[error("command execution was cancelled")]
    Cancelled,
    #[error("failed to spawn command '{0}': {1}")]
    SpawnFailed(String, std::io::Error),
    #[error("command failed with I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Execute a command with line-by-line streaming into a [`BoundedLog`], timeout, and cancellation.
///
/// # Errors
/// Returns [`RunnerError::TimedOut`] if the duration elapses, [`RunnerError::Cancelled`] if the token fires,
/// or [`RunnerError::SpawnFailed`] if launching fails.
pub async fn run_command_streaming(
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
    cancel: &CancellationToken,
    log: &BoundedLog,
) -> Result<ProcessOutput, RunnerError> {
    let mut cmd = create_argv_command(program, args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| RunnerError::SpawnFailed(program.display().to_string(), e))?;

    let pid = child.id();
    let mut tree_guard = ProcessTreeGuard::new(pid);

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let log_stdout = log.clone();
    let stdout_task = crate::util::spawn_in_span(async move {
        let mut full = String::new();
        if let Some(pipe) = stdout_pipe {
            let mut reader = tokio::io::BufReader::new(pipe).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log_stdout.push(LogStream::Stdout, line.clone());
                full.push_str(&line);
                full.push('\n');
            }
        }
        full
    });

    let log_stderr = log.clone();
    let stderr_task = crate::util::spawn_in_span(async move {
        let mut full = String::new();
        if let Some(pipe) = stderr_pipe {
            let mut reader = tokio::io::BufReader::new(pipe).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                log_stderr.push(LogStream::Stderr, line.clone());
                full.push_str(&line);
                full.push('\n');
            }
        }
        full
    });

    let wait_res = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            kill_process_tree(pid).await;
            tree_guard.disarm();
            if let Err(e) = child.wait().await {
                tracing::debug!(error = %e, "error reaping child after cancellation");
            }
            return Err(RunnerError::Cancelled);
        }
        () = tokio::time::sleep(timeout) => {
            kill_process_tree(pid).await;
            tree_guard.disarm();
            if let Err(e) = child.wait().await {
                tracing::debug!(error = %e, "error reaping child after timeout");
            }
            return Err(RunnerError::TimedOut(timeout));
        }
        status = child.wait() => status,
    };

    tree_guard.disarm();
    let status = wait_res?;

    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();

    Ok(ProcessOutput {
        exit_code: status.code(),
        success: status.success(),
        stdout,
        stderr,
    })
}

async fn detect_tool(bin: &Path, timeout: Duration, cancel: &CancellationToken) -> ToolStatus {
    let dummy_log = BoundedLog::new(10);
    match run_command_streaming(bin, &["--version"], None, timeout, cancel, &dummy_log).await {
        Ok(out) if out.success => ToolStatus {
            found: true,
            version: Some(out.stdout.trim().to_string()),
            path: Some(bin.display().to_string()),
        },
        _ => ToolStatus {
            found: false,
            version: None,
            path: None,
        },
    }
}

/// Detect environment prerequisites for Teams setup.
pub async fn detect_prereqs(
    residuum_root: &Path,
    agent_name: &str,
    overrides: Option<&AtkRunnerOverrides>,
) -> TeamsSetupPrereqs {
    let cancel = CancellationToken::new();
    let timeout = Duration::from_secs(10);

    // 1. Detect node
    let node_bin = overrides
        .and_then(|o| o.node_bin.as_deref())
        .unwrap_or_else(|| Path::new("node"));
    let node_status = detect_tool(node_bin, timeout, &cancel).await;

    // 2. Detect npm
    let npm_bin = overrides
        .and_then(|o| o.npm_bin.as_deref())
        .unwrap_or_else(|| Path::new("npm"));
    let npm_status = detect_tool(npm_bin, timeout, &cancel).await;

    // 3. Detect atk
    let paths = resolve_atk_paths(residuum_root, agent_name);
    let install_dir = hub_dir(residuum_root)
        .join("tools")
        .join("m365agentstoolkit");
    let atk_bin = overrides
        .and_then(|o| o.atk_bin.as_deref())
        .unwrap_or(paths.atk_bin.as_path());

    let dummy_log = BoundedLog::new(10);
    let atk_status = if atk_bin.exists() {
        match run_command_streaming(atk_bin, &["--version"], None, timeout, &cancel, &dummy_log)
            .await
        {
            Ok(out) if out.success => AtkStatus {
                installed: true,
                version: Some(out.stdout.trim().to_string()),
                path: Some(atk_bin.display().to_string()),
                pinned_version: ATK_CLI_VERSION.to_string(),
                install_dir: install_dir.display().to_string(),
            },
            _ => AtkStatus {
                installed: true,
                version: Some(ATK_CLI_VERSION.to_string()),
                path: Some(atk_bin.display().to_string()),
                pinned_version: ATK_CLI_VERSION.to_string(),
                install_dir: install_dir.display().to_string(),
            },
        }
    } else {
        AtkStatus {
            installed: false,
            version: None,
            path: None,
            pinned_version: ATK_CLI_VERSION.to_string(),
            install_dir: install_dir.display().to_string(),
        }
    };

    // 4. Check if teams is already configured for this agent
    let teams_already_configured = is_teams_configured_for_agent(residuum_root, agent_name).await;

    // 5. Derive suggested cloud endpoint
    let suggested = derive_cloud_teams_endpoint(residuum_root, agent_name).await;
    let suggested_endpoint_source = suggested
        .as_ref()
        .map(|_| SuggestedEndpointSource::ResiduumCloud);

    TeamsSetupPrereqs {
        node: node_status,
        npm: npm_status,
        atk: atk_status,
        min_node_version: MIN_NODE_VERSION.to_string(),
        teams_already_configured,
        suggested_endpoint: suggested,
        suggested_endpoint_source,
        manual_guide_url: MANUAL_GUIDE_URL.to_string(),
    }
}

async fn is_teams_configured_for_agent(residuum_root: &Path, agent_name: &str) -> bool {
    let agent_path = agent_dir(residuum_root, agent_name);
    let config_path = agent_path.join("config").join("config.toml");
    let legacy_config = agent_path.join("config.toml");
    let target = if config_path.exists() {
        config_path
    } else if legacy_config.exists() {
        legacy_config
    } else {
        return false;
    };

    let Ok(content) = tokio::fs::read_to_string(target).await else {
        return false;
    };
    content.contains("[teams]")
}

/// Check if the user is already signed in to Microsoft 365 via `atk auth list m365`.
pub async fn check_signed_in(atk_bin: &Path, log: &BoundedLog, cancel: &CancellationToken) -> bool {
    let timeout = Duration::from_secs(15);
    match run_command_streaming(
        atk_bin,
        &["auth", "list", "m365"],
        None,
        timeout,
        cancel,
        log,
    )
    .await
    {
        Ok(out) if out.success => {
            let stripped = strip_ansi_codes(&out.stdout);
            stripped.contains("Your Microsoft 365 account is:")
                || (stripped.contains("account is:") && !stripped.contains("auth login m365"))
        }
        _ => false,
    }
}

/// Install the pinned Microsoft 365 Agents Toolkit CLI into `install_dir` using npm.
///
/// # Errors
/// Returns [`SetupError`] if npm fails or the install command exits with an error.
pub async fn install_cli(
    install_dir: &Path,
    npm_bin: &Path,
    log: &BoundedLog,
    cancel: &CancellationToken,
) -> Result<(), SetupError> {
    tokio::fs::create_dir_all(install_dir)
        .await
        .map_err(|e| SetupError {
            phase: TeamsSetupPhase::InstallCli,
            message: format!(
                "Failed to create tools directory at {}: {e}",
                install_dir.display()
            ),
            detail: Some(e.to_string()),
        })?;

    log.push(
        LogStream::Info,
        format!(
            "Installing @microsoft/m365agentstoolkit-cli@{ATK_CLI_VERSION} in {}...",
            install_dir.display()
        ),
    );

    let prefix_str = install_dir.to_string_lossy().to_string();
    let package_arg = format!("@microsoft/m365agentstoolkit-cli@{ATK_CLI_VERSION}");
    let args = ["install", "--prefix", &prefix_str, &package_arg];

    let timeout = Duration::from_secs(300);
    let res = run_command_streaming(npm_bin, &args, Some(install_dir), timeout, cancel, log).await;

    match res {
        Ok(out) if out.success => {
            log.push(
                LogStream::Info,
                "Microsoft 365 Agents Toolkit CLI installed successfully.".to_string(),
            );
            Ok(())
        }
        Ok(out) => {
            let message = map_npm_error(&out.stderr, out.exit_code);
            Err(SetupError {
                phase: TeamsSetupPhase::InstallCli,
                message,
                detail: Some(out.stderr),
            })
        }
        Err(RunnerError::TimedOut(dur)) => Err(SetupError {
            phase: TeamsSetupPhase::InstallCli,
            message: format!(
                "Installing Microsoft 365 Agents Toolkit timed out after {} seconds.",
                dur.as_secs()
            ),
            detail: None,
        }),
        Err(RunnerError::Cancelled) => Err(SetupError {
            phase: TeamsSetupPhase::InstallCli,
            message: "Installation was cancelled by user.".to_string(),
            detail: None,
        }),
        Err(e) => Err(SetupError {
            phase: TeamsSetupPhase::InstallCli,
            message: format!("Failed to execute npm: {e}"),
            detail: Some(e.to_string()),
        }),
    }
}

/// Forward a pasted OAuth redirect URL to the local ATK listener, validating that the URL's port
/// matches `expected_port`.
///
/// # Errors
/// Returns [`FatalError::Config`] if port mismatch or network forward fails.
pub async fn forward_redirect_with_port_check(
    url_str: &str,
    expected_port: u16,
) -> Result<u16, FatalError> {
    let parsed = url::Url::parse(url_str)
        .map_err(|e| FatalError::Config(format!("Invalid redirect URL: not a valid URL: {e}")))?;

    let host = parsed.host_str().unwrap_or("");
    if host != "localhost" && host != "127.0.0.1" {
        return Err(FatalError::Config(format!(
            "Invalid redirect URL: must be a localhost URL (e.g. http://localhost:{expected_port}/)."
        )));
    }

    let port = parsed.port().ok_or_else(|| {
        FatalError::Config(format!(
            "Invalid redirect URL: wrong port (expected http://localhost:{expected_port}/, no port specified)."
        ))
    })?;

    if port != expected_port {
        return Err(FatalError::Config(format!(
            "Invalid redirect URL: wrong port (expected http://localhost:{expected_port}/, got port {port})."
        )));
    }

    forward_redirect(url_str).await
}

/// Detached agent-path login helper management.
pub struct AgentLoginManager;

impl AgentLoginManager {
    /// Start `atk auth login m365` detached, waiting until the sign-in URL appears.
    ///
    /// Writes PID to `<project_dir>/atk-login.pid` and logs output to `<project_dir>/atk-login.log`.
    ///
    /// # Errors
    /// Returns [`FatalError::Config`] if spawning fails, child dies early, or URL cannot be extracted.
    pub async fn start(
        residuum_root: &Path,
        agent_name: &str,
    ) -> Result<(String, u16), FatalError> {
        let paths = resolve_atk_paths(residuum_root, agent_name);
        tokio::fs::create_dir_all(&paths.project_dir)
            .await
            .map_err(|e| FatalError::Config(format!("failed to create project dir: {e}")))?;

        let pid_path = paths.project_dir.join("atk-login.pid");
        let log_path = paths.project_dir.join("atk-login.log");

        // Clean up stale pid if not running
        if pid_path.exists() {
            if let Ok(pid_str) = tokio::fs::read_to_string(&pid_path).await
                && let Ok(pid) = pid_str.trim().parse::<u32>()
                && is_process_running(pid)
            {
                // Check if URL is already available in log
                if let Ok(content) = tokio::fs::read_to_string(&log_path).await {
                    for line in content.lines() {
                        if let Some((url, port)) = parse_login_url_and_port(line) {
                            return Ok((url, port));
                        }
                    }
                }
                return Err(FatalError::Config(
                    "a login process is already running for this agent; use --cancel to terminate it or --status to check progress".to_string()
                ));
            }
            if let Err(e) = tokio::fs::remove_file(&pid_path).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = %e, path = %pid_path.display(), "failed to remove stale pid file");
            }
        }

        let log_file = std::fs::File::create(&log_path).map_err(|e| {
            FatalError::Config(format!(
                "failed to create login log file at {}: {e}",
                log_path.display()
            ))
        })?;

        let mut cmd = create_argv_command(&paths.atk_bin, &["auth", "login", "m365"]);
        cmd.stdout(
            log_file
                .try_clone()
                .map_err(|e| FatalError::Config(e.to_string()))?,
        );
        cmd.stderr(log_file);

        let child = cmd.spawn().map_err(|e| {
            FatalError::Config(format!(
                "failed to spawn '{} auth login m365': {e}",
                paths.atk_bin.display()
            ))
        })?;

        let pid = child
            .id()
            .ok_or_else(|| FatalError::Config("failed to get child pid".to_string()))?;
        tokio::fs::write(&pid_path, pid.to_string())
            .await
            .map_err(|e| FatalError::Config(format!("failed to write pid file: {e}")))?;

        // Poll log file for login URL up to 30 seconds
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while tokio::time::Instant::now() < deadline {
            if !is_process_running(pid) {
                let tail = tokio::fs::read_to_string(&log_path)
                    .await
                    .unwrap_or_default();
                return Err(FatalError::Config(format!(
                    "login process exited prematurely. Output:\n{tail}"
                )));
            }

            if let Ok(content) = tokio::fs::read_to_string(&log_path).await {
                for line in content.lines() {
                    if let Some((url, port)) = parse_login_url_and_port(line) {
                        return Ok((url, port));
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        Err(FatalError::Config(
            "timed out waiting for Microsoft sign-in URL to appear in login log".to_string(),
        ))
    }

    /// Check the status of detached login for an agent.
    pub async fn status(residuum_root: &Path, agent_name: &str) -> String {
        let paths = resolve_atk_paths(residuum_root, agent_name);
        let pid_path = paths.project_dir.join("atk-login.pid");
        let log_path = paths.project_dir.join("atk-login.log");

        if !pid_path.exists() {
            return "not running".to_string();
        }

        let pid = match tokio::fs::read_to_string(&pid_path).await {
            Ok(s) => match s.trim().parse::<u32>() {
                Ok(p) => p,
                Err(_) => return "not running (invalid pid file)".to_string(),
            },
            Err(_) => return "not running".to_string(),
        };

        if is_process_running(pid) {
            if let Ok(content) = tokio::fs::read_to_string(&log_path).await {
                for line in content.lines() {
                    if let Some((url, port)) = parse_login_url_and_port(line) {
                        return format!("waiting for user sign-in\n  URL:  {url}\n  Port: {port}");
                    }
                }
            }
            return format!("running (pid {pid}), waiting for login URL");
        }

        // Process is not running; check if signed in or failed
        let dummy_log = BoundedLog::new(10);
        let cancel = CancellationToken::new();
        if check_signed_in(&paths.atk_bin, &dummy_log, &cancel).await {
            if let Err(e) = tokio::fs::remove_file(&pid_path).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = %e, path = %pid_path.display(), "failed to remove stale pid file");
            }
            return "signed in".to_string();
        }

        let tail = tokio::fs::read_to_string(&log_path)
            .await
            .unwrap_or_else(|_| "(no log available)".to_string());
        format!("failed (process exited)\nLog tail:\n{tail}")
    }

    /// Cancel a running detached login process for an agent.
    ///
    /// # Errors
    /// Returns [`FatalError`] if cancellation fails.
    pub async fn cancel(residuum_root: &Path, agent_name: &str) -> Result<bool, FatalError> {
        let paths = resolve_atk_paths(residuum_root, agent_name);
        let pid_path = paths.project_dir.join("atk-login.pid");

        if !pid_path.exists() {
            return Ok(false);
        }

        if let Ok(pid_str) = tokio::fs::read_to_string(&pid_path).await
            && let Ok(pid) = pid_str.trim().parse::<u32>()
        {
            kill_process_tree_sync(Some(pid));
        }

        if let Err(e) = tokio::fs::remove_file(&pid_path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, path = %pid_path.display(), "failed to remove stale pid file");
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_log_seq_and_bounding() {
        let log = BoundedLog::new(3);
        assert_eq!(log.last_seq(), 0);

        let l1 = log.push(LogStream::Info, "line 1".to_string());
        assert_eq!(l1.seq, 1);
        assert_eq!(log.last_seq(), 1);

        let l2 = log.push(LogStream::Stdout, "line 2".to_string());
        assert_eq!(l2.seq, 2);

        let l3 = log.push(LogStream::Stderr, "line 3".to_string());
        assert_eq!(l3.seq, 3);

        // Lines since seq 1
        let since1 = log.lines_since(1);
        assert_eq!(since1.len(), 2);
        assert_eq!(since1.first().map(|l| l.text.as_str()), Some("line 2"));
        assert_eq!(since1.get(1).map(|l| l.text.as_str()), Some("line 3"));

        // Bounded capacity: line 4 evicts line 1
        let l4 = log.push(LogStream::Stdout, "line 4".to_string());
        assert_eq!(l4.seq, 4);

        let tail = log.tail(10);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail.first().map(|l| l.text.as_str()), Some("line 2"));
        assert_eq!(tail.get(1).map(|l| l.text.as_str()), Some("line 3"));
        assert_eq!(tail.get(2).map(|l| l.text.as_str()), Some("line 4"));
    }

    #[test]
    fn parse_login_url_and_port_real_atk_sample() {
        let line = "Log in to your Microsoft 365 account - opening default web browser at https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=test-id&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A35437%2F&prompt=select_account#";
        let (url, port) = parse_login_url_and_port(line).unwrap();
        assert_eq!(port, 35437);
        assert!(url.starts_with("https://login.microsoftonline.com"));
        assert!(!url.ends_with('#'));
    }

    #[test]
    fn parse_login_url_and_port_with_ansi_escapes() {
        let line = "\x1b[32mLog in to your Microsoft 365 account - opening default web browser at \x1b[4m\x1b[34mhttps://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=abc&redirect_uri=http%3A%2F%2F127.0.0.1%3A42424%2F\x1b[39m\x1b[24m#";
        let (url, port) = parse_login_url_and_port(line).unwrap();
        assert_eq!(port, 42424);
        assert!(url.contains("client_id=abc"));
    }

    #[test]
    fn parse_login_url_ignores_unrelated_lines() {
        assert_eq!(parse_login_url_and_port("Just a regular log line"), None);
        assert_eq!(
            parse_login_url_and_port("Use atk auth login to sign in"),
            None
        );
    }

    #[test]
    fn map_npm_error_patterns() {
        let err_net = map_npm_error(
            "npm ERR! code ENOTFOUND\nnpm ERR! network getaddrinfo failed",
            Some(1),
        );
        assert!(err_net.contains("Could not connect to the npm registry"));

        let err_perm = map_npm_error("npm ERR! code EACCES\nnpm ERR! permission denied", Some(1));
        assert!(err_perm.contains("Permission denied"));

        let err_other = map_npm_error("Some random syntax error", Some(2));
        assert!(err_other.contains("exit code 2"));
    }

    #[test]
    fn map_install_app_error_patterns() {
        let err_sideload = map_install_app_error(
            "Error: SideLoadingDisabled - tenant admin has disabled custom app upload",
        );
        assert!(err_sideload.contains("IT administrator for approval"));

        let err_normal = map_install_app_error("Zip file corrupt");
        assert!(err_normal.contains("Failed to install app in Microsoft Teams: Zip file corrupt"));
    }

    #[tokio::test]
    async fn forward_redirect_with_port_check_validates() {
        // Wrong port rejected
        let err = forward_redirect_with_port_check("http://localhost:3000/?code=123", 4000)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("wrong port"));

        // Non-localhost rejected
        let err_host = forward_redirect_with_port_check("http://example.com:3000/?code=123", 3000)
            .await
            .unwrap_err();
        assert!(err_host.to_string().contains("must be a localhost URL"));
    }
}
