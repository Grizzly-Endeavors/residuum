//! Protocol types for the Microsoft Teams setup wizard via Agents Toolkit.
//!
//! These types define the contract between the web client and the Teams setup
//! job runner.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToolStatus {
    pub found: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AtkStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub pinned_version: String,
    pub install_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SuggestedEndpointSource {
    ResiduumCloud,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TeamsSetupPrereqs {
    pub node: ToolStatus,
    pub npm: ToolStatus,
    pub atk: AtkStatus,
    pub min_node_version: String,
    pub teams_already_configured: bool,
    pub suggested_endpoint: Option<String>,
    pub suggested_endpoint_source: Option<SuggestedEndpointSource>,
    pub manual_guide_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TeamsSetupForm {
    pub bot_name: String,
    pub short_description: String,
    pub long_description: String,
    pub developer_name: String,
    pub developer_url: String,
    pub privacy_url: Option<String>,
    pub terms_url: Option<String>,
    pub messaging_endpoint: String,
    pub color_icon_png_base64: Option<String>,
    pub outline_icon_png_base64: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TeamsSetupStart {
    pub form: TeamsSetupForm,
    pub consent_install_cli: bool,
    pub replace_existing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TeamsSetupPhase {
    CheckPrereqs,
    InstallCli,
    SignIn,
    Scaffold,
    Provision,
    Import,
    InstallApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TeamsSetupState {
    Running,
    WaitingForUser,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LogStream {
    Stdout,
    Stderr,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogLine {
    #[ts(type = "number")]
    pub seq: u64,
    pub stream: LogStream,
    pub text: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SignInPrompt {
    pub login_url: String,
    #[ts(type = "number")]
    pub redirect_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetupError {
    pub phase: TeamsSetupPhase,
    pub message: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreatedResources {
    pub bot_id: Option<String>,
    pub teams_app_id: Option<String>,
    pub tenant_id: Option<String>,
    pub entra_url: String,
    pub dev_portal_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetupResult {
    pub bot_id: String,
    pub tenant_id: String,
    pub teams_app_id: Option<String>,
    pub package_path: String,
    pub project_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TeamsSetupJob {
    pub agent: String,
    pub state: TeamsSetupState,
    pub phase: TeamsSetupPhase,
    pub completed_phases: Vec<TeamsSetupPhase>,
    pub started_at: String,
    pub phase_started_at: String,
    pub sign_in: Option<SignInPrompt>,
    pub log: Vec<LogLine>,
    #[ts(type = "number")]
    pub last_seq: u64,
    pub error: Option<SetupError>,
    pub created: CreatedResources,
    pub result: Option<SetupResult>,
    pub app_installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TeamsSetupRedirect {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CleanupRequest {
    pub project_files: bool,
    pub cli: bool,
    pub sign_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CleanupFailure {
    pub item: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CleanupResult {
    pub removed: Vec<String>,
    pub failed: Vec<CleanupFailure>,
}

impl Default for CreatedResources {
    fn default() -> Self {
        Self {
            bot_id: None,
            teams_app_id: None,
            tenant_id: None,
            entra_url: "https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationMenuBlade/~/Overview/appId/".to_string(),
            dev_portal_url: "https://dev.teams.microsoft.com/apps/".to_string(),
        }
    }
}
