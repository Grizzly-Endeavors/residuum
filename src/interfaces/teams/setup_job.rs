//! In-memory setup job runner for Microsoft 365 Agents Toolkit integration.
//!
//! Manages phase execution, process streaming, interactive sign-in prompts,
//! cancellation, retries, and cleanup per the Teams setup API contract.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use base64::Engine as _;
use tokio::io::AsyncBufReadExt;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::paths::{agent_dir, hub_dir};
use crate::interfaces::teams::atk::{
    ATK_CLI_VERSION, AtkScaffoldOptions, import_atk_project, resolve_atk_paths,
    scaffold_atk_project, validate_png_dimensions,
};
use crate::interfaces::teams::atk_runner::{
    AtkRunnerOverrides, BoundedLog, check_signed_in, detect_prereqs,
    forward_redirect_with_port_check, install_cli, map_install_app_error, parse_login_url_and_port,
    run_command_streaming,
};
use crate::interfaces::teams::setup_types::{
    CleanupFailure, CleanupRequest, CleanupResult, CreatedResources, LogStream, SetupError,
    SetupResult, SignInPrompt, TeamsSetupForm, TeamsSetupJob, TeamsSetupPhase, TeamsSetupPrereqs,
    TeamsSetupStart, TeamsSetupState,
};
use crate::util::process::{create_argv_command, kill_process_tree_sync};

/// Errors encountered when managing Teams setup jobs.
#[derive(Debug, thiserror::Error)]
pub enum SetupJobError {
    /// A job is already active and running for this agent.
    #[error("job is already running for this agent")]
    Conflict(Box<TeamsSetupJob>),
    /// No job exists for the specified agent.
    #[error("no setup job found for agent '{0}'")]
    NotFound(String),
    /// Request validation failed.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// An internal execution failure occurred.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Internal shared state for an agent's setup job.
struct JobSession {
    agent: String,
    state: TeamsSetupState,
    phase: TeamsSetupPhase,
    completed_phases: Vec<TeamsSetupPhase>,
    started_at: String,
    phase_started_at: String,
    sign_in: Option<SignInPrompt>,
    log: BoundedLog,
    error: Option<SetupError>,
    created: CreatedResources,
    result: Option<SetupResult>,
    app_installed: bool,
    form: TeamsSetupForm,
    replace_existing: bool,
    active_cancel: CancellationToken,
    login_pid: Option<u32>,
}

impl JobSession {
    fn to_external(&self, log_since: Option<u64>) -> TeamsSetupJob {
        let since = log_since.unwrap_or(0);
        let log_lines = self.log.lines_since(since);
        let last_seq = self.log.last_seq();
        TeamsSetupJob {
            agent: self.agent.clone(),
            state: self.state,
            phase: self.phase,
            completed_phases: self.completed_phases.clone(),
            started_at: self.started_at.clone(),
            phase_started_at: self.phase_started_at.clone(),
            sign_in: self.sign_in.clone(),
            log: log_lines,
            last_seq,
            error: self.error.clone(),
            created: self.created.clone(),
            result: self.result.clone(),
            app_installed: self.app_installed,
        }
    }
}

/// Gateway-level manager holding one in-memory Teams setup job per agent.
#[derive(Clone)]
pub struct TeamsSetupJobManager {
    residuum_root: PathBuf,
    jobs: Arc<RwLock<HashMap<String, Arc<Mutex<JobSession>>>>>,
    overrides: Option<AtkRunnerOverrides>,
}

static MANAGERS: LazyLock<std::sync::Mutex<HashMap<PathBuf, TeamsSetupJobManager>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// Get or create a cached singleton `TeamsSetupJobManager` for the given residuum root.
#[must_use]
pub fn get_or_create_manager(residuum_root: &Path) -> TeamsSetupJobManager {
    let mut map = MANAGERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(residuum_root.to_path_buf())
        .or_insert_with(|| TeamsSetupJobManager::new(residuum_root.to_path_buf()))
        .clone()
}

/// Register a test manager with overrides for a specific residuum root.
pub fn register_test_manager(residuum_root: &Path, manager: TeamsSetupJobManager) {
    let mut map = MANAGERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.insert(residuum_root.to_path_buf(), manager);
}

impl TeamsSetupJobManager {
    /// Create a new setup job manager.
    #[must_use]
    pub fn new(residuum_root: PathBuf) -> Self {
        Self {
            residuum_root,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            overrides: None,
        }
    }

    /// Create a new setup job manager with injected executable overrides (for testing).
    #[must_use]
    pub fn with_overrides(residuum_root: PathBuf, overrides: AtkRunnerOverrides) -> Self {
        Self {
            residuum_root,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            overrides: Some(overrides),
        }
    }

    /// Retrieve prerequisite and context information for `agent`.
    pub async fn get_prereqs(&self, agent: &str) -> TeamsSetupPrereqs {
        detect_prereqs(&self.residuum_root, agent, self.overrides.as_ref()).await
    }

    /// Inspect current setup job status for `agent`.
    pub async fn get_job(&self, agent: &str, log_since: Option<u64>) -> Option<TeamsSetupJob> {
        let session = {
            let guard = self.jobs.read().await;
            guard.get(agent).cloned()?
        };
        let lock = session.lock().await;
        Some(lock.to_external(log_since))
    }

    /// Start a new setup job for `agent`.
    ///
    /// # Errors
    /// Returns [`SetupJobError::Conflict`] if a job is already running, or
    /// [`SetupJobError::BadRequest`] if input validation fails.
    pub async fn start_job(
        &self,
        agent: &str,
        req: TeamsSetupStart,
    ) -> Result<TeamsSetupJob, SetupJobError> {
        self.validate_start_form(agent, &req).await?;

        let mut jobs_guard = self.jobs.write().await;
        if let Some(existing) = jobs_guard.get(agent) {
            let lock = existing.lock().await;
            if lock.state == TeamsSetupState::Running
                || lock.state == TeamsSetupState::WaitingForUser
            {
                return Err(SetupJobError::Conflict(Box::new(lock.to_external(None))));
            }
        }

        let now = chrono::Utc::now().to_rfc3339();
        let log = BoundedLog::default();
        log.push(
            LogStream::Info,
            format!("Starting Teams setup for agent '{agent}'..."),
        );

        let cancel_token = CancellationToken::new();
        let session = Arc::new(Mutex::new(JobSession {
            agent: agent.to_string(),
            state: TeamsSetupState::Running,
            phase: TeamsSetupPhase::CheckPrereqs,
            completed_phases: Vec::new(),
            started_at: now.clone(),
            phase_started_at: now,
            sign_in: None,
            log,
            error: None,
            created: CreatedResources::default(),
            result: None,
            app_installed: false,
            form: req.form,
            replace_existing: req.replace_existing,
            active_cancel: cancel_token.clone(),
            login_pid: None,
        }));

        jobs_guard.insert(agent.to_string(), Arc::clone(&session));
        drop(jobs_guard);

        // Spawn runner task
        let manager_clone = self.clone();
        let agent_name = agent.to_string();
        let session_clone = Arc::clone(&session);
        crate::util::spawn_in_span(async move {
            manager_clone
                .drive_job(&agent_name, session_clone, cancel_token)
                .await;
        });

        let lock = session.lock().await;
        Ok(lock.to_external(None))
    }

    async fn validate_start_form(
        &self,
        agent: &str,
        req: &TeamsSetupStart,
    ) -> Result<(), SetupJobError> {
        let f = &req.form;
        if f.bot_name.trim().is_empty() {
            return Err(SetupJobError::BadRequest(
                "Bot name is required.".to_string(),
            ));
        }
        if f.short_description.trim().is_empty() {
            return Err(SetupJobError::BadRequest(
                "Short description is required.".to_string(),
            ));
        }
        if f.short_description.len() > 80 {
            return Err(SetupJobError::BadRequest(
                "Short description must be 80 characters or fewer.".to_string(),
            ));
        }
        if f.long_description.trim().is_empty() {
            return Err(SetupJobError::BadRequest(
                "Long description is required.".to_string(),
            ));
        }
        if f.long_description.len() > 4000 {
            return Err(SetupJobError::BadRequest(
                "Long description must be 4000 characters or fewer.".to_string(),
            ));
        }
        if f.developer_name.trim().is_empty() {
            return Err(SetupJobError::BadRequest(
                "Developer name is required.".to_string(),
            ));
        }
        if f.developer_url.trim().is_empty() {
            return Err(SetupJobError::BadRequest(
                "Developer URL is required.".to_string(),
            ));
        }
        if f.messaging_endpoint.trim().is_empty() {
            return Err(SetupJobError::BadRequest(
                "Messaging endpoint is required.".to_string(),
            ));
        }

        // Validate consent & replace prerequisites
        let prereqs = self.get_prereqs(agent).await;
        if !prereqs.atk.installed && !req.consent_install_cli {
            return Err(SetupJobError::BadRequest(
                "consent_install_cli is required: You must consent to installing the Agents Toolkit CLI to continue.".to_string(),
            ));
        }
        if prereqs.teams_already_configured && !req.replace_existing {
            return Err(SetupJobError::BadRequest(
                "replace_existing is required: Microsoft Teams is already configured. Confirm replacing the existing bot to continue."
                    .to_string(),
            ));
        }

        Ok(())
    }

    /// Forward a pasted OAuth redirect URL to the waiting sign-in listener.
    ///
    /// # Errors
    /// Returns [`SetupJobError::BadRequest`] if port doesn't match or listener fails.
    pub async fn forward_redirect(
        &self,
        agent: &str,
        url_str: &str,
    ) -> Result<TeamsSetupJob, SetupJobError> {
        let session = {
            let guard = self.jobs.read().await;
            guard
                .get(agent)
                .cloned()
                .ok_or_else(|| SetupJobError::NotFound(agent.to_string()))?
        };

        let expected_port = {
            let lock = session.lock().await;
            if lock.state != TeamsSetupState::WaitingForUser
                || lock.phase != TeamsSetupPhase::SignIn
            {
                return Err(SetupJobError::BadRequest(
                    "Job is not waiting for a sign-in redirect.".to_string(),
                ));
            }
            let prompt = lock.sign_in.as_ref().ok_or_else(|| {
                SetupJobError::BadRequest(
                    "Authentication listener is no longer running.".to_string(),
                )
            })?;
            prompt.redirect_port
        };

        forward_redirect_with_port_check(url_str, expected_port)
            .await
            .map_err(|e| SetupJobError::BadRequest(e.to_string()))?;

        let lock = session.lock().await;
        Ok(lock.to_external(None))
    }

    /// Cancel a running or waiting setup job.
    ///
    /// # Errors
    /// Returns [`SetupJobError::NotFound`] if no job exists for `agent`.
    pub async fn cancel_job(&self, agent: &str) -> Result<TeamsSetupJob, SetupJobError> {
        let session = {
            let guard = self.jobs.read().await;
            guard
                .get(agent)
                .cloned()
                .ok_or_else(|| SetupJobError::NotFound(agent.to_string()))?
        };

        let mut lock = session.lock().await;
        if lock.state == TeamsSetupState::Running || lock.state == TeamsSetupState::WaitingForUser {
            lock.active_cancel.cancel();
            if let Some(pid) = lock.login_pid.take() {
                kill_process_tree_sync(Some(pid));
            }
            lock.state = TeamsSetupState::Cancelled;
            lock.log
                .push(LogStream::Info, "Setup job cancelled by user.".to_string());
        }

        Ok(lock.to_external(None))
    }

    /// Retry a failed or cancelled setup job from the failed phase.
    ///
    /// # Errors
    /// Returns [`SetupJobError::NotFound`] if no job exists for `agent`, or
    /// [`SetupJobError::Conflict`] if the job is already running.
    pub async fn retry_job(&self, agent: &str) -> Result<TeamsSetupJob, SetupJobError> {
        let session = {
            let guard = self.jobs.read().await;
            guard
                .get(agent)
                .cloned()
                .ok_or_else(|| SetupJobError::NotFound(agent.to_string()))?
        };

        let mut lock = session.lock().await;
        if lock.state == TeamsSetupState::Running || lock.state == TeamsSetupState::WaitingForUser {
            return Err(SetupJobError::Conflict(Box::new(lock.to_external(None))));
        }

        let cancel_token = CancellationToken::new();
        lock.active_cancel = cancel_token.clone();
        lock.state = TeamsSetupState::Running;
        lock.error = None;
        lock.phase_started_at = chrono::Utc::now().to_rfc3339();
        lock.log.push(
            LogStream::Info,
            format!("Retrying Teams setup job from phase '{:?}'...", lock.phase),
        );

        let manager_clone = self.clone();
        let agent_name = agent.to_string();
        let session_clone = Arc::clone(&session);
        crate::util::spawn_in_span(async move {
            manager_clone
                .drive_job(&agent_name, session_clone, cancel_token)
                .await;
        });

        Ok(lock.to_external(None))
    }

    /// Sideload the generated Teams app package into the user's tenant.
    ///
    /// # Errors
    /// Returns [`SetupJobError::NotFound`] if no job exists for `agent`, or
    /// [`SetupJobError::BadRequest`] if setup has not succeeded yet.
    pub async fn install_app(&self, agent: &str) -> Result<TeamsSetupJob, SetupJobError> {
        let session = {
            let guard = self.jobs.read().await;
            guard
                .get(agent)
                .cloned()
                .ok_or_else(|| SetupJobError::NotFound(agent.to_string()))?
        };

        let mut initial_lock = session.lock().await;
        if initial_lock.state != TeamsSetupState::Succeeded {
            return Err(SetupJobError::BadRequest(
                "Teams setup must be completed successfully before installing the app".to_string(),
            ));
        }

        initial_lock.phase = TeamsSetupPhase::InstallApp;
        initial_lock.state = TeamsSetupState::Running;
        initial_lock.phase_started_at = chrono::Utc::now().to_rfc3339();
        initial_lock.log.push(
            LogStream::Info,
            "Installing app package in Microsoft Teams...".to_string(),
        );

        let cancel = CancellationToken::new();
        initial_lock.active_cancel = cancel.clone();

        let paths = resolve_atk_paths(&self.residuum_root, agent);
        let atk_bin = self
            .overrides
            .as_ref()
            .and_then(|o| o.atk_bin.clone())
            .unwrap_or(paths.atk_bin);

        let package_path_str = paths.package_zip.to_string_lossy().to_string();
        let log = initial_lock.log.clone();
        drop(initial_lock);

        let res = run_command_streaming(
            &atk_bin,
            &["install", "--file-path", &package_path_str],
            None,
            Duration::from_secs(180),
            &cancel,
            &log,
        )
        .await;

        let mut result_lock = session.lock().await;
        match res {
            Ok(out) if out.success => {
                result_lock.app_installed = true;
                result_lock.state = TeamsSetupState::Succeeded;
                result_lock.log.push(
                    LogStream::Info,
                    "Teams app installed successfully!".to_string(),
                );
            }
            Ok(out) => {
                let msg = map_install_app_error(&out.stderr);
                result_lock.state = TeamsSetupState::Failed;
                result_lock.error = Some(SetupError {
                    phase: TeamsSetupPhase::InstallApp,
                    message: msg.clone(),
                    detail: Some(out.stderr),
                });
                tracing::error!(agent = %agent, phase = "install_app", error = %msg, "teams setup job failed");
            }
            Err(e) => {
                let msg = format!("Failed to run atk install: {e}");
                result_lock.state = TeamsSetupState::Failed;
                result_lock.error = Some(SetupError {
                    phase: TeamsSetupPhase::InstallApp,
                    message: msg.clone(),
                    detail: Some(e.to_string()),
                });
                tracing::error!(agent = %agent, phase = "install_app", error = %msg, "teams setup job failed");
            }
        }

        Ok(result_lock.to_external(None))
    }

    /// Return path to built `appPackage.residuum.zip`.
    ///
    /// # Errors
    /// Returns [`SetupJobError::NotFound`] if the package zip file does not exist.
    pub fn package_path(&self, agent: &str) -> Result<PathBuf, SetupJobError> {
        let paths = resolve_atk_paths(&self.residuum_root, agent);
        if paths.package_zip.is_file() {
            Ok(paths.package_zip)
        } else {
            Err(SetupJobError::NotFound(format!(
                "app package zip not found at {}",
                paths.package_zip.display()
            )))
        }
    }

    /// Delete finished job from memory.
    ///
    /// # Errors
    /// Returns [`SetupJobError::Conflict`] if the job is currently running.
    pub async fn delete_job(&self, agent: &str) -> Result<(), SetupJobError> {
        let mut guard = self.jobs.write().await;
        if let Some(session) = guard.get(agent) {
            let lock = session.lock().await;
            if lock.state == TeamsSetupState::Running
                || lock.state == TeamsSetupState::WaitingForUser
            {
                return Err(SetupJobError::Conflict(Box::new(lock.to_external(None))));
            }
        }
        guard.remove(agent);
        Ok(())
    }

    /// Clean up local setup files, CLI installation, and/or Microsoft 365 sign-out.
    pub async fn cleanup(&self, agent: &str, req: CleanupRequest) -> CleanupResult {
        let mut removed = Vec::new();
        let mut failed = Vec::new();

        if req.project_files {
            let project_dir = agent_dir(&self.residuum_root, agent).join("teams-app");
            if project_dir.exists() {
                if let Err(e) = tokio::fs::remove_dir_all(&project_dir).await {
                    failed.push(CleanupFailure {
                        item: "project_files".to_string(),
                        message: format!("failed to remove {}: {e}", project_dir.display()),
                    });
                } else {
                    removed.push("project_files".to_string());
                }
            } else {
                removed.push("project_files".to_string());
            }
        }

        if req.cli {
            let cli_dir = hub_dir(&self.residuum_root)
                .join("tools")
                .join("m365agentstoolkit");
            if cli_dir.exists() {
                if let Err(e) = tokio::fs::remove_dir_all(&cli_dir).await {
                    failed.push(CleanupFailure {
                        item: "cli".to_string(),
                        message: format!("failed to remove {}: {e}", cli_dir.display()),
                    });
                } else {
                    removed.push("cli".to_string());
                }
            } else {
                removed.push("cli".to_string());
            }
        }

        if req.sign_out {
            let paths = resolve_atk_paths(&self.residuum_root, agent);
            let atk_bin = self
                .overrides
                .as_ref()
                .and_then(|o| o.atk_bin.clone())
                .unwrap_or(paths.atk_bin);
            if atk_bin.exists() {
                let dummy_log = BoundedLog::new(10);
                let cancel = CancellationToken::new();
                let res = run_command_streaming(
                    &atk_bin,
                    &["auth", "logout", "m365"],
                    None,
                    Duration::from_secs(30),
                    &cancel,
                    &dummy_log,
                )
                .await;
                match res {
                    Ok(out) if out.success => {
                        removed.push("sign_out".to_string());
                    }
                    Ok(out) => {
                        failed.push(CleanupFailure {
                            item: "sign_out".to_string(),
                            message: format!("logout exited with error: {}", out.stderr),
                        });
                    }
                    Err(e) => {
                        failed.push(CleanupFailure {
                            item: "sign_out".to_string(),
                            message: format!("failed to execute logout: {e}"),
                        });
                    }
                }
            } else {
                removed.push("sign_out".to_string());
            }
        }

        CleanupResult { removed, failed }
    }

    /// Background runner task driving the job through its phases.
    async fn drive_job(
        &self,
        agent: &str,
        session: Arc<Mutex<JobSession>>,
        cancel: CancellationToken,
    ) {
        loop {
            if cancel.is_cancelled() {
                let mut lock = session.lock().await;
                lock.state = TeamsSetupState::Cancelled;
                return;
            }

            let phase = {
                let lock = session.lock().await;
                if lock.state != TeamsSetupState::Running {
                    return;
                }
                lock.phase
            };

            let step_result = match phase {
                TeamsSetupPhase::CheckPrereqs => self.step_check_prereqs(agent, &session).await,
                TeamsSetupPhase::InstallCli => {
                    self.step_install_cli(agent, &session, &cancel).await
                }
                TeamsSetupPhase::SignIn => self.step_sign_in(agent, &session, &cancel).await,
                TeamsSetupPhase::Scaffold => self.step_scaffold(agent, &session).await,
                TeamsSetupPhase::Provision => self.step_provision(agent, &session, &cancel).await,
                TeamsSetupPhase::Import => self.step_import(agent, &session).await,
                TeamsSetupPhase::InstallApp => Ok(()),
            };

            match step_result {
                Ok(()) => {
                    let mut lock = session.lock().await;
                    if !lock.completed_phases.contains(&phase) {
                        lock.completed_phases.push(phase);
                    }
                    if let Some(next) = next_phase(phase) {
                        lock.phase = next;
                        lock.phase_started_at = chrono::Utc::now().to_rfc3339();
                    } else {
                        lock.state = TeamsSetupState::Succeeded;
                        return;
                    }
                }
                Err(err) => {
                    let mut lock = session.lock().await;
                    tracing::error!(
                        agent = %agent,
                        phase = ?phase,
                        error = %err.message,
                        "teams setup job failed"
                    );
                    lock.error = Some(err);
                    lock.state = TeamsSetupState::Failed;
                    return;
                }
            }
        }
    }

    async fn step_check_prereqs(
        &self,
        agent: &str,
        session: &Arc<Mutex<JobSession>>,
    ) -> Result<(), SetupError> {
        let prereqs = self.get_prereqs(agent).await;
        let lock = session.lock().await;
        lock.log.push(
            LogStream::Info,
            "Validating system prerequisites...".to_string(),
        );

        if !prereqs.node.found {
            return Err(SetupError {
                phase: TeamsSetupPhase::CheckPrereqs,
                message: "Node.js is not installed. Please install Node.js (version 18+ LTS recommended) and ensure it is in PATH.".to_string(),
                detail: None,
            });
        }

        if !prereqs.npm.found {
            return Err(SetupError {
                phase: TeamsSetupPhase::CheckPrereqs,
                message: "npm is not installed. Please install npm and ensure it is in PATH."
                    .to_string(),
                detail: None,
            });
        }

        lock.log.push(
            LogStream::Info,
            format!(
                "Node.js ({}) and npm ({}) found.",
                prereqs.node.version.as_deref().unwrap_or("ok"),
                prereqs.npm.version.as_deref().unwrap_or("ok")
            ),
        );
        Ok(())
    }

    async fn step_install_cli(
        &self,
        agent: &str,
        session: &Arc<Mutex<JobSession>>,
        cancel: &CancellationToken,
    ) -> Result<(), SetupError> {
        let (prereqs, log) = {
            let lock = session.lock().await;
            (self.get_prereqs(agent).await, lock.log.clone())
        };

        if prereqs.atk.installed {
            log.push(
                LogStream::Info,
                format!("Microsoft 365 Agents Toolkit ({ATK_CLI_VERSION}) is already installed."),
            );
            return Ok(());
        }

        let install_dir = hub_dir(&self.residuum_root)
            .join("tools")
            .join("m365agentstoolkit");
        let default_npm = if cfg!(windows) {
            PathBuf::from("npm.cmd")
        } else {
            PathBuf::from("npm")
        };
        let npm_bin = self
            .overrides
            .as_ref()
            .and_then(|o| o.npm_bin.clone())
            .unwrap_or(default_npm);

        install_cli(&install_dir, &npm_bin, &log, cancel).await
    }

    async fn step_sign_in(
        &self,
        agent: &str,
        session: &Arc<Mutex<JobSession>>,
        cancel: &CancellationToken,
    ) -> Result<(), SetupError> {
        let paths = resolve_atk_paths(&self.residuum_root, agent);
        let atk_bin = self
            .overrides
            .as_ref()
            .and_then(|o| o.atk_bin.clone())
            .unwrap_or(paths.atk_bin);

        let log = {
            let lock = session.lock().await;
            lock.log.clone()
        };

        if check_signed_in(&atk_bin, &log, cancel).await {
            log.push(
                LogStream::Info,
                "Already signed in to Microsoft 365.".to_string(),
            );
            return Ok(());
        }

        log.push(
            LogStream::Info,
            "Initiating interactive Microsoft 365 sign-in...".to_string(),
        );

        let mut spawned = spawn_login_process(&atk_bin, &log).await?;
        {
            let mut lock = session.lock().await;
            lock.login_pid = spawned.pid;
            lock.state = TeamsSetupState::WaitingForUser;
            lock.sign_in = Some(spawned.prompt);
            lock.log.push(
                LogStream::Info,
                "Awaiting Microsoft 365 browser authentication...".to_string(),
            );
        }

        let wait_res = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                kill_process_tree_sync(spawned.pid);
                drop(spawned.child.wait().await);
                return Err(SetupError {
                    phase: TeamsSetupPhase::SignIn,
                    message: "Sign-in was cancelled by user.".to_string(),
                    detail: None,
                });
            }
            () = tokio::time::sleep(Duration::from_mins(15)) => {
                kill_process_tree_sync(spawned.pid);
                drop(spawned.child.wait().await);
                return Err(SetupError {
                    phase: TeamsSetupPhase::SignIn,
                    message: "Sign-in timed out after 15 minutes. Please try again.".to_string(),
                    detail: None,
                });
            }
            status = spawned.child.wait() => status,
        };

        let status = wait_res.map_err(|e| SetupError {
            phase: TeamsSetupPhase::SignIn,
            message: format!("Error awaiting sign-in process: {e}"),
            detail: Some(e.to_string()),
        })?;

        drop(spawned.stdout_task.await);
        let stderr_out = spawned.stderr_task.await.unwrap_or_default();

        let mut lock = session.lock().await;
        lock.login_pid = None;
        lock.sign_in = None;

        if !status.success() {
            return Err(SetupError {
                phase: TeamsSetupPhase::SignIn,
                message: format!(
                    "Microsoft 365 sign-in failed (exit code {}).",
                    status.code().unwrap_or(-1)
                ),
                detail: Some(stderr_out),
            });
        }

        lock.state = TeamsSetupState::Running;
        lock.log.push(
            LogStream::Info,
            "Microsoft 365 sign-in completed successfully.".to_string(),
        );
        Ok(())
    }

    async fn step_scaffold(
        &self,
        agent: &str,
        session: &Arc<Mutex<JobSession>>,
    ) -> Result<(), SetupError> {
        let (form, replace_existing, log) = {
            let lock = session.lock().await;
            (lock.form.clone(), lock.replace_existing, lock.log.clone())
        };

        log.push(
            LogStream::Info,
            "Scaffolding Teams project files and icons...".to_string(),
        );

        let temp_dir = tempfile::tempdir().map_err(|e| SetupError {
            phase: TeamsSetupPhase::Scaffold,
            message: format!("Failed to create temporary directory for icons: {e}"),
            detail: Some(e.to_string()),
        })?;

        let color_icon = if let Some(ref b64) = form.color_icon_png_base64 {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| SetupError {
                    phase: TeamsSetupPhase::Scaffold,
                    message: format!("Invalid color icon base64 encoding: {e}"),
                    detail: None,
                })?;
            validate_png_dimensions(&bytes, 192, 192, "color icon").map_err(|e| SetupError {
                phase: TeamsSetupPhase::Scaffold,
                message: e.to_string(),
                detail: None,
            })?;
            let path = temp_dir.path().join("color.png");
            tokio::fs::write(&path, &bytes)
                .await
                .map_err(|e| SetupError {
                    phase: TeamsSetupPhase::Scaffold,
                    message: format!("Failed to write color icon: {e}"),
                    detail: Some(e.to_string()),
                })?;
            Some(path)
        } else {
            None
        };

        let outline_icon = if let Some(ref b64) = form.outline_icon_png_base64 {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| SetupError {
                    phase: TeamsSetupPhase::Scaffold,
                    message: format!("Invalid outline icon base64 encoding: {e}"),
                    detail: None,
                })?;
            validate_png_dimensions(&bytes, 32, 32, "outline icon").map_err(|e| SetupError {
                phase: TeamsSetupPhase::Scaffold,
                message: e.to_string(),
                detail: None,
            })?;
            let path = temp_dir.path().join("outline.png");
            tokio::fs::write(&path, &bytes)
                .await
                .map_err(|e| SetupError {
                    phase: TeamsSetupPhase::Scaffold,
                    message: format!("Failed to write outline icon: {e}"),
                    detail: Some(e.to_string()),
                })?;
            Some(path)
        } else {
            None
        };

        let options = AtkScaffoldOptions {
            agent_name: agent.to_string(),
            endpoint: form.messaging_endpoint,
            project_dir: None,
            force: replace_existing,
            bot_name: Some(form.bot_name),
            developer_name: Some(form.developer_name),
            developer_url: Some(form.developer_url),
            privacy_url: form.privacy_url,
            terms_url: form.terms_url,
            short_description: Some(form.short_description),
            long_description: Some(form.long_description),
            color_icon,
            outline_icon,
        };

        scaffold_atk_project(&self.residuum_root, &options)
            .await
            .map_err(|e| SetupError {
                phase: TeamsSetupPhase::Scaffold,
                message: format!("Failed to scaffold project: {e}"),
                detail: Some(e.to_string()),
            })?;

        log.push(
            LogStream::Info,
            "Project scaffolded successfully.".to_string(),
        );
        Ok(())
    }

    async fn step_provision(
        &self,
        agent: &str,
        session: &Arc<Mutex<JobSession>>,
        cancel: &CancellationToken,
    ) -> Result<(), SetupError> {
        let paths = resolve_atk_paths(&self.residuum_root, agent);
        let atk_bin = self
            .overrides
            .as_ref()
            .and_then(|o| o.atk_bin.clone())
            .unwrap_or(paths.atk_bin);

        let log = {
            let lock = session.lock().await;
            lock.log.clone()
        };

        log.push(
            LogStream::Info,
            "Provisioning Microsoft Teams resources in Microsoft 365...".to_string(),
        );

        let folder_str = paths.project_dir.to_string_lossy().to_string();
        let args = [
            "provision",
            "--folder",
            &folder_str,
            "--env",
            "residuum",
            "--interactive",
            "false",
        ];

        let res = run_command_streaming(
            &atk_bin,
            &args,
            Some(&paths.project_dir),
            Duration::from_secs(300),
            cancel,
            &log,
        )
        .await;

        // Read created resources from .env.residuum regardless of provision success or failure
        self.update_created_resources_from_disk(agent, session)
            .await;

        match res {
            Ok(out) if out.success => {
                log.push(
                    LogStream::Info,
                    "Resource provisioning completed successfully.".to_string(),
                );
                Ok(())
            }
            Ok(out) => {
                let err_msg = format!(
                    "Provisioning failed (exit code {}).",
                    out.exit_code.unwrap_or(-1)
                );
                Err(SetupError {
                    phase: TeamsSetupPhase::Provision,
                    message: err_msg,
                    detail: Some(out.stderr),
                })
            }
            Err(e) => Err(SetupError {
                phase: TeamsSetupPhase::Provision,
                message: format!("Provisioning command failed: {e}"),
                detail: Some(e.to_string()),
            }),
        }
    }

    async fn update_created_resources_from_disk(
        &self,
        agent: &str,
        session: &Arc<Mutex<JobSession>>,
    ) {
        let env_file = agent_dir(&self.residuum_root, agent)
            .join("teams-app")
            .join("env")
            .join(".env.residuum");

        if let Ok(content) = tokio::fs::read_to_string(&env_file).await {
            let mut bot_id = None;
            let mut tenant_id = None;
            let mut teams_app_id = None;

            for line in content.lines() {
                let trimmed = line.trim();
                if let Some((k, v)) = trimmed.split_once('=') {
                    let key = k.trim();
                    let val = v.trim().trim_matches(['"', '\'']);
                    if !val.is_empty() {
                        if key == "BOT_ID" {
                            bot_id = Some(val.to_string());
                        } else if key == "TEAMS_APP_TENANT_ID" {
                            tenant_id = Some(val.to_string());
                        } else if key == "TEAMS_APP_ID" {
                            teams_app_id = Some(val.to_string());
                        }
                    }
                }
            }

            let mut lock = session.lock().await;
            if bot_id.is_some() {
                lock.created.bot_id = bot_id;
            }
            if tenant_id.is_some() {
                lock.created.tenant_id = tenant_id;
            }
            if teams_app_id.is_some() {
                lock.created.teams_app_id = teams_app_id;
            }
        }
    }

    async fn step_import(
        &self,
        agent: &str,
        session: &Arc<Mutex<JobSession>>,
    ) -> Result<(), SetupError> {
        let paths = resolve_atk_paths(&self.residuum_root, agent);
        let log = {
            let lock = session.lock().await;
            lock.log.clone()
        };

        log.push(
            LogStream::Info,
            "Importing secrets and configuring Teams adapter...".to_string(),
        );

        let res = import_atk_project(&paths.project_dir, agent, &self.residuum_root)
            .await
            .map_err(|e| SetupError {
                phase: TeamsSetupPhase::Import,
                message: format!("Failed to import ATK project credentials: {e}"),
                detail: Some(e.to_string()),
            })?;

        let mut lock = session.lock().await;
        lock.result = Some(SetupResult {
            bot_id: res.bot_id.clone(),
            tenant_id: res.tenant_id.clone(),
            teams_app_id: res.teams_app_id.clone(),
            package_path: paths.package_zip.to_string_lossy().to_string(),
            project_dir: paths.project_dir.to_string_lossy().to_string(),
        });
        lock.created.bot_id = Some(res.bot_id);
        lock.created.tenant_id = Some(res.tenant_id);
        if res.teams_app_id.is_some() {
            lock.created.teams_app_id = res.teams_app_id;
        }

        log.push(
            LogStream::Info,
            "Teams bot credentials and configuration applied successfully!".to_string(),
        );
        Ok(())
    }
}

fn next_phase(current: TeamsSetupPhase) -> Option<TeamsSetupPhase> {
    match current {
        TeamsSetupPhase::CheckPrereqs => Some(TeamsSetupPhase::InstallCli),
        TeamsSetupPhase::InstallCli => Some(TeamsSetupPhase::SignIn),
        TeamsSetupPhase::SignIn => Some(TeamsSetupPhase::Scaffold),
        TeamsSetupPhase::Scaffold => Some(TeamsSetupPhase::Provision),
        TeamsSetupPhase::Provision => Some(TeamsSetupPhase::Import),
        TeamsSetupPhase::Import | TeamsSetupPhase::InstallApp => None,
    }
}

struct LoginSpawn {
    child: tokio::process::Child,
    pid: Option<u32>,
    prompt: SignInPrompt,
    stdout_task: tokio::task::JoinHandle<String>,
    stderr_task: tokio::task::JoinHandle<String>,
}

async fn spawn_login_process(atk_bin: &Path, log: &BoundedLog) -> Result<LoginSpawn, SetupError> {
    let mut cmd = create_argv_command(atk_bin, &["auth", "login", "m365"]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| SetupError {
        phase: TeamsSetupPhase::SignIn,
        message: format!("Failed to spawn 'atk auth login m365': {e}"),
        detail: Some(e.to_string()),
    })?;

    let pid = child.id();
    let mut stdout_reader =
        tokio::io::BufReader::new(child.stdout.take().ok_or_else(|| SetupError {
            phase: TeamsSetupPhase::SignIn,
            message: "Failed to open login stdout pipe".to_string(),
            detail: None,
        })?)
        .lines();

    let mut stderr_reader =
        tokio::io::BufReader::new(child.stderr.take().ok_or_else(|| SetupError {
            phase: TeamsSetupPhase::SignIn,
            message: "Failed to open login stderr pipe".to_string(),
            detail: None,
        })?)
        .lines();

    let log_err = log.clone();
    let stderr_task = crate::util::spawn_in_span(async move {
        let mut buf = String::new();
        while let Ok(Some(line)) = stderr_reader.next_line().await {
            log_err.push(LogStream::Stderr, line.clone());
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    let mut prompt_found = None;
    while let Ok(Some(line)) = stdout_reader.next_line().await {
        log.push(LogStream::Stdout, line.clone());
        if let Some((url, port)) = parse_login_url_and_port(&line) {
            prompt_found = Some(SignInPrompt {
                login_url: url,
                redirect_port: port,
            });
            break;
        }
    }

    let prompt = prompt_found.ok_or_else(|| SetupError {
        phase: TeamsSetupPhase::SignIn,
        message: "Sign-in URL was not produced by login process.".to_string(),
        detail: None,
    })?;

    let log_out = log.clone();
    let stdout_task = crate::util::spawn_in_span(async move {
        let mut buf = String::new();
        while let Ok(Some(line)) = stdout_reader.next_line().await {
            log_out.push(LogStream::Stdout, line.clone());
            buf.push_str(&line);
            buf.push('\n');
        }
        buf
    });

    Ok(LoginSpawn {
        child,
        pid,
        prompt,
        stdout_task,
        stderr_task,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}

    fn make_script(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap();
        make_executable(path);
    }

    const FAKE_ATK_UNIX: &str = r#"#!/bin/sh
cmd="$1"
sub="$2"
if [ "$cmd" = "--version" ]; then
    echo "1.1.17"
    exit 0
fi
if [ "$cmd" = "auth" ] && [ "$sub" = "list" ]; then
    echo "Your Microsoft 365 account is: test@contoso.com."
    exit 0
fi
if [ "$cmd" = "auth" ] && [ "$sub" = "logout" ]; then
    echo "Logged out."
    exit 0
fi
if [ "$cmd" = "provision" ]; then
    folder=""
    while [ "$#" -gt 0 ]; do
        if [ "$1" = "--folder" ]; then
            folder="$2"
            shift 2
        else
            shift
        fi
    done
    mkdir -p "$folder/env"
    cat << 'EOF' > "$folder/env/.env.residuum"
BOT_ID=bot-mock-1234
TEAMS_APP_TENANT_ID=tenant-mock-5678
TEAMS_APP_ID=app-mock-9999
EOF
    cat << 'EOF' > "$folder/env/.env.residuum.user"
SECRET_BOT_PASSWORD=mock-pass
EOF
    mkdir -p "$folder/appPackage/build"
    echo "zip" > "$folder/appPackage/build/appPackage.residuum.zip"
    echo "Provisioning succeeded"
    exit 0
fi
if [ "$cmd" = "install" ]; then
    echo "App installed successfully"
    exit 0
fi
exit 0
"#;

    const FAKE_ATK_WIN_PS1: &str = r#"$cmd = $args[0]
$sub = $args[1]
if ($cmd -eq "--version") { Write-Output "1.1.17"; exit 0 }
if ($cmd -eq "auth" -and $sub -eq "list") { Write-Output "Your Microsoft 365 account is: test@contoso.com."; exit 0 }
if ($cmd -eq "auth" -and $sub -eq "logout") { Write-Output "Logged out."; exit 0 }
if ($cmd -eq "install") { Write-Output "App installed successfully"; exit 0 }
if ($cmd -eq "provision") {
    $folder = ""
    for ($i = 0; $i -lt $args.Length; $i++) {
        if ($args[$i] -eq "--folder" -and $i + 1 -lt $args.Length) {
            $folder = $args[$i + 1]
            break
        }
    }
    if ($folder -ne "") {
        New-Item -ItemType Directory -Force -Path "$folder\env" | Out-Null
        $envRes = @"
BOT_ID=bot-mock-1234
TEAMS_APP_TENANT_ID=tenant-mock-5678
TEAMS_APP_ID=app-mock-9999
"@
        [System.IO.File]::WriteAllText("$folder\env\.env.residuum", $envRes)
        [System.IO.File]::WriteAllText("$folder\env\.env.residuum.user", "SECRET_BOT_PASSWORD=mock-pass`n")
        New-Item -ItemType Directory -Force -Path "$folder\appPackage\build" | Out-Null
        [System.IO.File]::WriteAllText("$folder\appPackage\build\appPackage.residuum.zip", "zip`n")
        Write-Output "Provisioning succeeded"
    }
    exit 0
}
exit 0
"#;

    const SLOW_ATK_UNIX: &str = r#"#!/bin/sh
cmd="$1"
sub="$2"
if [ "$cmd" = "--version" ]; then echo "1.1.17"; exit 0; fi
if [ "$cmd" = "auth" ] && [ "$sub" = "list" ]; then echo "Your Microsoft 365 account is: test@contoso.com."; exit 0; fi
if [ "$cmd" = "provision" ]; then
    folder=""
    while [ "$#" -gt 0 ]; do
        if [ "$1" = "--folder" ]; then folder="$2"; shift 2; else shift; fi
    done
    sleep 30 &
    echo $! > "$folder/grandchild.pid"
    wait
    exit 0
fi
exit 0
"#;

    const SLOW_ATK_WIN_PS1: &str = r#"$cmd = $args[0]
$sub = $args[1]
if ($cmd -eq "--version") { Write-Output "1.1.17"; exit 0 }
if ($cmd -eq "auth" -and $sub -eq "list") { Write-Output "Your Microsoft 365 account is: test@contoso.com."; exit 0 }
if ($cmd -eq "provision") {
    $folder = ""
    for ($i = 0; $i -lt $args.Length; $i++) {
        if ($args[$i] -eq "--folder" -and $i + 1 -lt $args.Length) {
            $folder = $args[$i + 1]
            break
        }
    }
    $p = Start-Process powershell -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 30' -PassThru
    [System.IO.File]::WriteAllText((Join-Path $folder 'grandchild.pid'), $p.Id.ToString())
    Wait-Process -Id $p.Id
    exit 0
}
exit 0
"#;

    const LOGIN_ATK_UNIX: &str = r#"#!/bin/sh
cmd="$1"
sub="$2"
if [ "$cmd" = "--version" ]; then echo "1.1.17"; exit 0; fi
if [ "$cmd" = "auth" ] && [ "$sub" = "list" ]; then
    echo "No account signed in." >&2
    exit 1
fi
if [ "$cmd" = "auth" ] && [ "$sub" = "login" ]; then
    python3 -c '
import http.server, socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
port = s.getsockname()[1]
s.close()
print(f"Log in to your Microsoft 365 account - opening default web browser at https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=123&redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2F", flush=True)

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"OK")
    def log_message(self, format, *args):
        pass

server = http.server.HTTPServer(("127.0.0.1", port), Handler)
server.handle_request()
'
    exit 0
fi
if [ "$cmd" = "provision" ]; then
    folder=""
    while [ "$#" -gt 0 ]; do
        if [ "$1" = "--folder" ]; then folder="$2"; shift 2; else shift; fi
    done
    mkdir -p "$folder/env"
    cat << 'EOF' > "$folder/env/.env.residuum"
BOT_ID=bot-login-1111
TEAMS_APP_TENANT_ID=tenant-login-2222
TEAMS_APP_ID=app-login-3333
EOF
    cat << 'EOF' > "$folder/env/.env.residuum.user"
SECRET_BOT_PASSWORD=mock-pass
EOF
    mkdir -p "$folder/appPackage/build"
    echo "zip" > "$folder/appPackage/build/appPackage.residuum.zip"
    exit 0
fi
exit 0
"#;

    const LOGIN_ATK_WIN_PS1: &str = r#"$cmd = $args[0]
$sub = $args[1]
if ($cmd -eq "--version") { Write-Output "1.1.17"; exit 0 }
if ($cmd -eq "auth" -and $sub -eq "list") {
    [Console]::Error.WriteLine("No account signed in.")
    exit 1
}
if ($cmd -eq "auth" -and $sub -eq "login") {
    $s = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $s.Start()
    $port = $s.LocalEndpoint.Port
    Write-Output "Log in to your Microsoft 365 account - opening default web browser at https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=123&redirect_uri=http%3A%2F%2Flocalhost%3A$port%2F"
    $client = $s.AcceptTcpClient()
    $stream = $client.GetStream()
    $reader = [System.IO.StreamReader]::new($stream)
    $null = $reader.ReadLine()
    $response = [System.Text.Encoding]::UTF8.GetBytes("HTTP/1.1 200 OK`r`nContent-Length: 2`r`nConnection: close`r`n`r`nOK")
    $stream.Write($response, 0, $response.Length)
    $stream.Flush()
    $client.Close()
    $s.Stop()
    exit 0
}
if ($cmd -eq "provision") {
    $folder = ""
    for ($i = 0; $i -lt $args.Length; $i++) {
        if ($args[$i] -eq "--folder" -and $i + 1 -lt $args.Length) {
            $folder = $args[$i + 1]
            break
        }
    }
    if ($folder -ne "") {
        New-Item -ItemType Directory -Force -Path "$folder\env" | Out-Null
        $envRes = @"
BOT_ID=bot-login-1111
TEAMS_APP_TENANT_ID=tenant-login-2222
TEAMS_APP_ID=app-login-3333
"@
        [System.IO.File]::WriteAllText("$folder\env\.env.residuum", $envRes)
        [System.IO.File]::WriteAllText("$folder\env\.env.residuum.user", "SECRET_BOT_PASSWORD=mock-pass`n")
        New-Item -ItemType Directory -Force -Path "$folder\appPackage\build" | Out-Null
        [System.IO.File]::WriteAllText("$folder\appPackage\build\appPackage.residuum.zip", "zip`n")
    }
    exit 0
}
exit 0
"#;

    const FAIL_ATK_UNIX: &str = r#"#!/bin/sh
cmd="$1"
sub="$2"
if [ "$cmd" = "--version" ]; then echo "1.1.17"; exit 0; fi
if [ "$cmd" = "auth" ] && [ "$sub" = "list" ]; then echo "Your Microsoft 365 account is: test@contoso.com."; exit 0; fi
if [ "$cmd" = "provision" ]; then
    folder=""
    while [ "$#" -gt 0 ]; do
        if [ "$1" = "--folder" ]; then folder="$2"; shift 2; else shift; fi
    done
    mkdir -p "$folder/env"
    echo "BOT_ID=partial-bot-id-4444" > "$folder/env/.env.residuum"
    echo "fatal provisioning error" >&2
    exit 1
fi
exit 0
"#;

    const FAIL_ATK_WIN_PS1: &str = r#"$cmd = $args[0]
$sub = $args[1]
if ($cmd -eq "--version") { Write-Output "1.1.17"; exit 0 }
if ($cmd -eq "auth" -and $sub -eq "list") { Write-Output "Your Microsoft 365 account is: test@contoso.com."; exit 0 }
if ($cmd -eq "provision") {
    $folder = ""
    for ($i = 0; $i -lt $args.Length; $i++) {
        if ($args[$i] -eq "--folder" -and $i + 1 -lt $args.Length) {
            $folder = $args[$i + 1]
            break
        }
    }
    if ($folder -ne "") {
        New-Item -ItemType Directory -Force -Path "$folder\env" | Out-Null
        [System.IO.File]::WriteAllText("$folder\env\.env.residuum", "BOT_ID=partial-bot-id-4444`n")
    }
    [Console]::Error.WriteLine("fatal provisioning error")
    exit 1
}
exit 0
"#;

    fn make_ps1_or_sh_script(path: &Path, unix_script: &str, win_ps1: &str) {
        if cfg!(windows) {
            let ps1_path = path.with_extension("ps1");
            std::fs::write(&ps1_path, win_ps1).unwrap();
            make_script(
                path,
                "@echo off\nif \"%~1\"==\"--version\" ( echo 1.1.17 & exit /b 0 )\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dpn0.ps1\" %*\nexit /b %ERRORLEVEL%\n",
            );
        } else {
            make_script(path, unix_script);
        }
    }

    fn make_fake_atk_script(path: &Path) {
        make_ps1_or_sh_script(path, FAKE_ATK_UNIX, FAKE_ATK_WIN_PS1);
    }

    fn make_slow_atk_script(path: &Path) {
        make_ps1_or_sh_script(path, SLOW_ATK_UNIX, SLOW_ATK_WIN_PS1);
    }

    fn make_login_atk_script(path: &Path) {
        make_ps1_or_sh_script(path, LOGIN_ATK_UNIX, LOGIN_ATK_WIN_PS1);
    }

    fn make_fail_atk_script(path: &Path) {
        make_ps1_or_sh_script(path, FAIL_ATK_UNIX, FAIL_ATK_WIN_PS1);
    }

    fn setup_mock_env(temp: &tempfile::TempDir) -> (PathBuf, AtkRunnerOverrides) {
        let root = temp.path().to_path_buf();
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();

        let ext = if cfg!(windows) { ".cmd" } else { "" };
        let node_path = bin_dir.join(format!("fake_node{ext}"));
        if cfg!(windows) {
            make_script(&node_path, "@echo off\necho v20.11.0\nexit /b 0\n");
        } else {
            make_script(&node_path, "#!/bin/sh\necho 'v20.11.0'\nexit 0\n");
        }

        let npm_path = bin_dir.join(format!("fake_npm{ext}"));
        if cfg!(windows) {
            make_script(
                &npm_path,
                "@echo off\nif \"%~1\"==\"--version\" ( echo 10.2.4 ) else ( echo ok )\nexit /b 0\n",
            );
        } else {
            make_script(
                &npm_path,
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '10.2.4'; else echo 'ok'; fi\nexit 0\n",
            );
        }

        let atk_path = bin_dir.join(format!("fake_atk{ext}"));
        make_fake_atk_script(&atk_path);

        let overrides = AtkRunnerOverrides {
            node_bin: Some(node_path),
            npm_bin: Some(npm_path),
            atk_bin: Some(atk_path),
        };
        (root, overrides)
    }

    fn sample_start_form() -> TeamsSetupStart {
        TeamsSetupStart {
            form: TeamsSetupForm {
                bot_name: "Test Bot".to_string(),
                messaging_endpoint: "https://example.com/api/teams".to_string(),
                developer_name: "Test Dev".to_string(),
                developer_url: "https://example.com".to_string(),
                privacy_url: None,
                terms_url: None,
                short_description: "A short description".to_string(),
                long_description: "A longer description".to_string(),
                color_icon_png_base64: None,
                outline_icon_png_base64: None,
            },
            consent_install_cli: true,
            replace_existing: false,
        }
    }

    #[tokio::test]
    async fn test_job_prereqs_detection() {
        let temp = tempfile::tempdir().unwrap();
        let (root, overrides) = setup_mock_env(&temp);
        let mgr = TeamsSetupJobManager::with_overrides(root, overrides);
        let prereqs = mgr.get_prereqs("agent-1").await;
        assert!(prereqs.node.found);
        assert_eq!(prereqs.node.version.as_deref(), Some("v20.11.0"));
        assert!(prereqs.npm.found);
        assert_eq!(prereqs.npm.version.as_deref(), Some("10.2.4"));
        assert!(prereqs.atk.installed);
    }

    #[tokio::test]
    async fn test_job_conflict_when_already_running() {
        let temp = tempfile::tempdir().unwrap();
        let (root, overrides) = setup_mock_env(&temp);
        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);

        // Create agent dir and hub config
        tokio::fs::create_dir_all(root.join("hub")).await.unwrap();
        tokio::fs::create_dir_all(root.join("agent-1"))
            .await
            .unwrap();
        tokio::fs::write(root.join("hub/config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();
        tokio::fs::write(
            root.join("agent-1/config.toml"),
            "[agent]\nname = \"agent-1\"\n",
        )
        .await
        .unwrap();

        let req = sample_start_form();
        let started = mgr.start_job("agent-1", req.clone()).await.unwrap();
        assert_eq!(started.agent, "agent-1");

        // Attempting to start second job while first is running returns Conflict
        let conflict = mgr.start_job("agent-1", req).await;
        assert!(matches!(conflict, Err(SetupJobError::Conflict(_))));
    }

    #[tokio::test]
    async fn test_job_full_happy_path() {
        let temp = tempfile::tempdir().unwrap();
        let (root, overrides) = setup_mock_env(&temp);
        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);

        // Create agent dir and hub config
        tokio::fs::create_dir_all(root.join("hub")).await.unwrap();
        tokio::fs::create_dir_all(root.join("agent-1"))
            .await
            .unwrap();
        tokio::fs::write(root.join("hub/config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();
        tokio::fs::write(
            root.join("agent-1/config.toml"),
            "[agent]\nname = \"agent-1\"\n",
        )
        .await
        .unwrap();

        let req = sample_start_form();
        let _ = mgr.start_job("agent-1", req).await.unwrap();

        // Poll job until Succeeded
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut final_job = None;
        while tokio::time::Instant::now() < deadline {
            if let Some(job) = mgr.get_job("agent-1", None).await
                && (job.state == TeamsSetupState::Succeeded || job.state == TeamsSetupState::Failed)
            {
                final_job = Some(job);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let job = final_job.expect("job should finish");
        assert_eq!(
            job.state,
            TeamsSetupState::Succeeded,
            "error: {:?}",
            job.error
        );
        assert_eq!(job.created.bot_id.as_deref(), Some("bot-mock-1234"));
        assert!(job.result.is_some());
        let res = job.result.unwrap();
        assert_eq!(res.bot_id, "bot-mock-1234");
        assert_eq!(res.tenant_id, "tenant-mock-5678");

        // Install app
        let install_res = mgr.install_app("agent-1").await.unwrap();
        assert!(install_res.app_installed);

        // Package path
        let pkg_path = mgr.package_path("agent-1").unwrap();
        assert!(pkg_path.ends_with("appPackage.residuum.zip"));

        // Delete job
        mgr.delete_job("agent-1").await.unwrap();
        assert!(mgr.get_job("agent-1", None).await.is_none());
    }

    #[tokio::test]
    async fn test_job_cancel_during_execution() {
        let temp = tempfile::tempdir().unwrap();
        let (root, mut overrides) = setup_mock_env(&temp);

        // Slow provision script that spawns a grandchild process
        let ext = if cfg!(windows) { ".cmd" } else { "" };
        let slow_atk = root.join(format!("bin/slow_atk{ext}"));
        make_slow_atk_script(&slow_atk);
        overrides.atk_bin = Some(slow_atk);

        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);
        tokio::fs::create_dir_all(root.join("hub")).await.unwrap();
        tokio::fs::create_dir_all(root.join("agent-1"))
            .await
            .unwrap();
        tokio::fs::write(root.join("hub/config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();
        tokio::fs::write(
            root.join("agent-1/config.toml"),
            "[agent]\nname = \"agent-1\"\n",
        )
        .await
        .unwrap();

        let req = sample_start_form();
        mgr.start_job("agent-1", req).await.unwrap();

        // Wait until it reaches Provision phase and grandchild.pid exists
        let pid_file = root.join("agent-1/teams-app/grandchild.pid");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut grandchild_pid = None;
        while tokio::time::Instant::now() < deadline {
            if let Some(j) = mgr.get_job("agent-1", None).await
                && j.phase == TeamsSetupPhase::Provision
                && let Ok(content) = tokio::fs::read_to_string(&pid_file).await
                && let Ok(pid) = content.trim().parse::<u32>()
            {
                grandchild_pid = Some(pid);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let pid = grandchild_pid.expect("grandchild should have spawned and written pid");
        assert!(crate::util::process::is_process_running(pid));

        let cancelled = mgr.cancel_job("agent-1").await.unwrap();
        assert_eq!(cancelled.state, TeamsSetupState::Cancelled);

        // Verify that the grandchild process tree is actually gone
        let kill_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < kill_deadline {
            if !crate::util::process::is_process_running(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !crate::util::process::is_process_running(pid),
            "grandchild process tree should be terminated after cancel"
        );
    }

    #[tokio::test]
    async fn test_job_sign_in_waiting_and_redirect_forwarding() {
        let temp = tempfile::tempdir().unwrap();
        let (root, mut overrides) = setup_mock_env(&temp);

        let ext = if cfg!(windows) { ".cmd" } else { "" };
        let login_atk = root.join(format!("bin/login_atk{ext}"));
        make_login_atk_script(&login_atk);
        overrides.atk_bin = Some(login_atk);

        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);
        tokio::fs::create_dir_all(root.join("hub")).await.unwrap();
        tokio::fs::create_dir_all(root.join("agent-1"))
            .await
            .unwrap();
        tokio::fs::write(root.join("hub/config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();
        tokio::fs::write(
            root.join("agent-1/config.toml"),
            "[agent]\nname = \"agent-1\"\n",
        )
        .await
        .unwrap();

        let req = sample_start_form();
        mgr.start_job("agent-1", req).await.unwrap();

        // Wait until it reaches SignIn phase and WaitingForUser state
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut sign_in_prompt = None;
        while tokio::time::Instant::now() < deadline {
            if let Some(j) = mgr.get_job("agent-1", None).await
                && j.phase == TeamsSetupPhase::SignIn
                && j.state == TeamsSetupState::WaitingForUser
                && let Some(p) = j.sign_in
            {
                sign_in_prompt = Some(p);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let prompt = sign_in_prompt.expect("job should enter sign_in waiting_for_user");
        let port = prompt.redirect_port;

        // Port mismatch rejection
        let wrong_port_url = format!("http://localhost:{}/?code=test", port + 1);
        let err_port = mgr
            .forward_redirect("agent-1", &wrong_port_url)
            .await
            .unwrap_err();
        assert!(matches!(
            err_port,
            SetupJobError::BadRequest(ref msg) if msg.contains("wrong port")
        ));

        // Host mismatch rejection
        let err_host = mgr
            .forward_redirect("agent-1", "https://example.com:4321/?code=test")
            .await
            .unwrap_err();
        assert!(matches!(
            err_host,
            SetupJobError::BadRequest(ref msg) if msg.contains("must be a localhost URL")
        ));

        // Valid redirect forward to the fake listener
        let valid_url = format!("http://localhost:{port}/?code=valid-code-xyz");
        mgr.forward_redirect("agent-1", &valid_url)
            .await
            .expect("forward_redirect should succeed with matching port");

        // The job should now advance past SignIn to Scaffold/Provision/Succeeded
        let advance_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut advanced = false;
        while tokio::time::Instant::now() < advance_deadline {
            if let Some(j) = mgr.get_job("agent-1", None).await
                && (j.phase != TeamsSetupPhase::SignIn || j.state == TeamsSetupState::Succeeded)
            {
                advanced = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(advanced, "job should advance past sign-in after redirect");
    }

    #[tokio::test]
    async fn test_job_failure_mid_provision_records_created_and_retry() {
        let temp = tempfile::tempdir().unwrap();
        let (root, mut overrides) = setup_mock_env(&temp);

        // Failing provision script that writes partial env file and exits with 1
        let ext = if cfg!(windows) { ".cmd" } else { "" };
        let fail_atk = root.join(format!("bin/fail_atk{ext}"));
        make_fail_atk_script(&fail_atk);
        overrides.atk_bin = Some(fail_atk.clone());

        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);
        tokio::fs::create_dir_all(root.join("hub")).await.unwrap();
        tokio::fs::create_dir_all(root.join("agent-1"))
            .await
            .unwrap();
        tokio::fs::write(root.join("hub/config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();
        tokio::fs::write(
            root.join("agent-1/config.toml"),
            "[agent]\nname = \"agent-1\"\n",
        )
        .await
        .unwrap();

        let req = sample_start_form();
        mgr.start_job("agent-1", req).await.unwrap();

        // Wait for failure
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut failed_job = None;
        while tokio::time::Instant::now() < deadline {
            if let Some(j) = mgr.get_job("agent-1", None).await
                && j.state == TeamsSetupState::Failed
            {
                failed_job = Some(j);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let job = failed_job.expect("job should have failed");
        assert_eq!(job.phase, TeamsSetupPhase::Provision);
        assert_eq!(job.created.bot_id.as_deref(), Some("partial-bot-id-4444"));
        assert!(job.error.is_some());

        // Now fix the script and retry
        make_fake_atk_script(&fail_atk);

        mgr.retry_job("agent-1").await.unwrap();

        // Wait for success
        let retry_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut succeeded_job = None;
        while tokio::time::Instant::now() < retry_deadline {
            if let Some(j) = mgr.get_job("agent-1", None).await
                && j.state == TeamsSetupState::Succeeded
            {
                succeeded_job = Some(j);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let retried = succeeded_job.expect("retried job should succeed");
        assert_eq!(retried.created.bot_id.as_deref(), Some("bot-mock-1234"));
    }

    #[tokio::test]
    async fn test_cleanup_files() {
        let temp = tempfile::tempdir().unwrap();
        let (root, overrides) = setup_mock_env(&temp);
        let mgr = TeamsSetupJobManager::with_overrides(root.clone(), overrides);

        let project_dir = root.join("agent-1/teams-app");
        let cli_dir = root.join("hub/tools/m365agentstoolkit");
        tokio::fs::create_dir_all(&project_dir).await.unwrap();
        tokio::fs::create_dir_all(&cli_dir).await.unwrap();

        let res = mgr
            .cleanup(
                "agent-1",
                CleanupRequest {
                    project_files: true,
                    cli: true,
                    sign_out: true,
                },
            )
            .await;

        assert!(!project_dir.exists());
        assert!(!cli_dir.exists());
        assert!(res.removed.contains(&"project_files".to_string()));
        assert!(res.removed.contains(&"cli".to_string()));
        assert!(res.removed.contains(&"sign_out".to_string()));
    }
}
