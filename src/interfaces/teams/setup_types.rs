//! Data types for Microsoft Teams setup wizard and job runner.
//!
//! Follows the Teams setup wizard API contract (v1).

use serde::{Deserialize, Serialize};

/// Installation and availability status for a required CLI tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatus {
    /// Whether the tool executable was found in PATH.
    pub found: bool,
    /// Discovered version string if available.
    pub version: Option<String>,
    /// Absolute path to the discovered executable if available.
    pub path: Option<String>,
}

/// Installation status for the Microsoft 365 Agents Toolkit CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtkStatus {
    /// Whether the CLI is installed in Residuum's tools directory.
    pub installed: bool,
    /// Discovered version of the installed CLI.
    pub version: Option<String>,
    /// Path to the installed ATK binary if found.
    pub path: Option<String>,
    /// Pinned version expected by this Residuum release.
    pub pinned_version: String,
    /// Directory where the CLI is installed.
    pub install_dir: String,
}

/// Prerequisite check results and setup environment context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamsSetupPrereqs {
    /// Node.js status.
    pub node: ToolStatus,
    /// npm status.
    pub npm: ToolStatus,
    /// Microsoft 365 Agents Toolkit status.
    pub atk: AtkStatus,
    /// Minimum Node.js version required by the toolkit.
    pub min_node_version: String,
    /// Whether Teams integration is already configured for this agent.
    pub teams_already_configured: bool,
    /// Suggested Teams messaging endpoint if derived.
    pub suggested_endpoint: Option<String>,
    /// Source of the suggested endpoint (e.g. "`residuum_cloud`").
    pub suggested_endpoint_source: Option<String>,
    /// Documentation URL for manual setup instructions.
    pub manual_guide_url: String,
}

/// Form parameters submitted to start Teams bot provisioning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamsSetupForm {
    /// Bot display name.
    pub bot_name: String,
    /// Short description for Teams app manifest (<= 80 chars).
    pub short_description: String,
    /// Long description for Teams app manifest (<= 4000 chars).
    pub long_description: String,
    /// Developer entity or person name.
    pub developer_name: String,
    /// Developer website URL.
    pub developer_url: String,
    /// Privacy policy URL.
    pub privacy_url: Option<String>,
    /// Terms of use URL.
    pub terms_url: Option<String>,
    /// Bot Framework messaging endpoint URL.
    pub messaging_endpoint: String,
    /// Optional base64-encoded 192x192 PNG color icon.
    pub color_icon_png_base64: Option<String>,
    /// Optional base64-encoded 32x32 PNG outline icon.
    pub outline_icon_png_base64: Option<String>,
}

/// Request payload to start a new Teams setup job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamsSetupStart {
    /// Bot details and manifest configuration.
    pub form: TeamsSetupForm,
    /// Explicit user consent to install the CLI locally if missing.
    pub consent_install_cli: bool,
    /// Confirmation to replace existing Teams bot configuration if present.
    pub replace_existing: bool,
}

/// Lifecycle phases in the Teams setup process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamsSetupPhase {
    /// Checking Node, npm, and environment prerequisites.
    CheckPrereqs,
    /// Installing pinned ATK CLI locally if missing.
    InstallCli,
    /// Signing in to Microsoft 365 tenant.
    SignIn,
    /// Scaffolding local project template files and icons.
    Scaffold,
    /// Running non-interactive ATK resource provisioning.
    Provision,
    /// Decrypting password, saving secrets, and writing config.
    Import,
    /// Sideloading the app package into Teams.
    InstallApp,
}

/// Current execution state of a setup job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamsSetupState {
    /// Actively executing commands or waiting for I/O.
    Running,
    /// Blocked awaiting user action (e.g. M365 browser sign-in).
    WaitingForUser,
    /// All setup phases completed successfully.
    Succeeded,
    /// Terminated due to error.
    Failed,
    /// Explicitly cancelled by user.
    Cancelled,
}

/// Stream identifier for captured log output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    /// Standard output from CLI process.
    Stdout,
    /// Standard error from CLI process.
    Stderr,
    /// Informational message from setup runner.
    Info,
}

/// Sequenced log entry captured during setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// Monotonically increasing sequence number (1-based).
    pub seq: usize,
    /// Originating stream.
    pub stream: LogStream,
    /// Content of the log line.
    pub text: String,
    /// RFC 3339 timestamp.
    pub at: String,
}

/// Instructions for browser-based Microsoft 365 login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInPrompt {
    /// MSAL login URL to visit in browser.
    pub login_url: String,
    /// Local redirect port waiting on daemon host.
    pub redirect_port: u16,
}

/// Error details captured when a setup phase fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupError {
    /// Phase in which failure occurred.
    pub phase: TeamsSetupPhase,
    /// Plain-language, actionable description of the failure.
    pub message: String,
    /// Additional technical or diagnostic context if available.
    pub detail: Option<String>,
}

/// Cloud and tenant resources created during setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedResources {
    /// Bot Application (Client) ID.
    pub bot_id: Option<String>,
    /// Teams App Definition ID.
    pub teams_app_id: Option<String>,
    /// Microsoft Entra Tenant ID.
    pub tenant_id: Option<String>,
    /// Microsoft Entra App Registrations portal link.
    pub entra_url: String,
    /// Microsoft Teams Developer Portal bot management link.
    pub dev_portal_url: String,
}

impl Default for CreatedResources {
    fn default() -> Self {
        Self {
            bot_id: None,
            teams_app_id: None,
            tenant_id: None,
            entra_url: "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade".to_string(),
            dev_portal_url: "https://dev.teams.microsoft.com/bots".to_string(),
        }
    }
}

/// Final output data once setup succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupResult {
    /// Bot Application (Client) ID.
    pub bot_id: String,
    /// Microsoft Entra Tenant ID.
    pub tenant_id: String,
    /// Teams App Definition ID.
    pub teams_app_id: Option<String>,
    /// Path to built zip package for sideloading.
    pub package_path: String,
    /// Path to local project directory.
    pub project_dir: String,
}

/// Active or completed setup job state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamsSetupJob {
    /// Target agent name.
    pub agent: String,
    /// Current execution state.
    pub state: TeamsSetupState,
    /// Current or last active phase.
    pub phase: TeamsSetupPhase,
    /// List of successfully completed phases.
    pub completed_phases: Vec<TeamsSetupPhase>,
    /// RFC 3339 timestamp when job was initiated.
    pub started_at: String,
    /// RFC 3339 timestamp when current phase started.
    pub phase_started_at: String,
    /// Sign-in prompt information while awaiting user auth.
    pub sign_in: Option<SignInPrompt>,
    /// Captured log lines matching caller filter.
    pub log: Vec<LogLine>,
    /// Sequence number of the most recent log line in runner.
    pub last_seq: usize,
    /// Error details if the job failed.
    pub error: Option<SetupError>,
    /// Tenant resources discovered or created.
    pub created: CreatedResources,
    /// Final setup results on success.
    pub result: Option<SetupResult>,
    /// Whether the Teams app package has been sideloaded into Teams.
    pub app_installed: bool,
}

/// Request parameters for cleaning up setup artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupRequest {
    /// Delete local project directory (<agent>/teams-app).
    pub project_files: bool,
    /// Delete local CLI installation (<hub>/tools/m365agentstoolkit).
    pub cli: bool,
    /// Run `atk auth logout m365` to clear token cache.
    pub sign_out: bool,
}

/// Per-item failure detail in cleanup response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupFailedItem {
    /// Item identifier (e.g. "`project_files`", "`cli`", "`sign_out`").
    pub item: String,
    /// Plain-language failure description.
    pub message: String,
}

/// Summary result of cleanup operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupResult {
    /// List of successfully removed items.
    pub removed: Vec<String>,
    /// List of items that failed cleanup with reasons.
    pub failed: Vec<CleanupFailedItem>,
}
