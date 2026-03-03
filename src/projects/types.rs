//! Project context data types.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// YAML frontmatter parsed from a `PROJECT.md` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFrontmatter {
    /// Human-readable project name.
    pub name: String,
    /// Brief summary of what this project covers.
    pub description: String,
    /// Whether the project is active or archived.
    pub status: ProjectStatus,
    /// When the project was created.
    pub created: NaiveDate,
    /// Tools to load when this project activates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// MCP server names to resolve from `mcp.json` when this project activates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    /// When the project was archived (only set for archived projects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<NaiveDate>,
}

/// Project lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStatus {
    /// Project is active and available for activation.
    Active,
    /// Project is archived and read-only.
    Archived,
}

/// Transport type for MCP server connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Stdio transport: spawn a child process and communicate over stdin/stdout.
    #[default]
    Stdio,
    /// HTTP transport: connect to a remote MCP server over Streamable HTTP.
    Http,
}

/// MCP server entry in project frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// Server name.
    pub name: String,
    /// Command (stdio) or URL (http) for the server.
    pub command: String,
    /// Command-line arguments (only used for stdio transport).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment variables to pass to the server process (only used for stdio transport).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Transport type (defaults to stdio).
    #[serde(default, skip_serializing_if = "is_default_transport")]
    pub transport: McpTransport,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if requires &T signature"
)]
fn is_default_transport(t: &McpTransport) -> bool {
    matches!(*t, McpTransport::Stdio)
}

/// Lightweight index entry for a project (frontmatter only, no body).
#[derive(Debug, Clone)]
pub struct ProjectIndexEntry {
    /// Human-readable project name.
    pub name: String,
    /// Brief description.
    pub description: String,
    /// Active or archived.
    pub status: ProjectStatus,
    /// Directory name on disk (sanitized).
    pub dir_name: String,
    /// Whether this project lives in the archive directory.
    pub is_archived: bool,
}

/// Fully loaded project context for an active project.
#[derive(Debug, Clone)]
pub struct ActiveProject {
    /// Human-readable project name.
    pub name: String,
    /// Directory name on disk.
    pub dir_name: String,
    /// Parsed frontmatter.
    pub frontmatter: ProjectFrontmatter,
    /// Markdown body from PROJECT.md (below the frontmatter).
    pub body: String,
    /// Recent session log content loaded on activation (most recent first).
    pub recent_log: Option<String>,
    /// File manifest of the project directory.
    pub manifest: ProjectManifest,
    /// Absolute path to the project root directory.
    pub project_root: PathBuf,
}

/// File manifest for a project's subdirectories.
#[derive(Debug, Clone, Default)]
pub struct ProjectManifest {
    /// Files under `notes/`.
    pub notes: Vec<ManifestEntry>,
    /// Files under `references/`.
    pub references: Vec<ManifestEntry>,
    /// Files under `workspace/`.
    pub workspace: Vec<ManifestEntry>,
    /// Files under `skills/`.
    pub skills: Vec<ManifestEntry>,
}

/// A single file entry in the manifest.
#[derive(Debug, Clone)]
pub struct ManifestEntry {
    /// Path relative to the project root.
    pub relative_path: String,
    /// File size in bytes.
    pub size_bytes: u64,
}

impl std::fmt::Display for ProjectStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Archived => write!(f, "archived"),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_serde_round_trip() {
        let fm = ProjectFrontmatter {
            name: "test-project".to_string(),
            description: "A test project".to_string(),
            status: ProjectStatus::Active,
            created: NaiveDate::from_ymd_opt(2026, 2, 10).unwrap(),
            tools: vec!["exec".to_string(), "read".to_string()],
            mcp_servers: vec![],
            archived: None,
        };

        let yaml = serde_yml::to_string(&fm).unwrap();
        let parsed: ProjectFrontmatter = serde_yml::from_str(&yaml).unwrap();

        assert_eq!(parsed.name, "test-project", "name should round-trip");
        assert_eq!(
            parsed.status,
            ProjectStatus::Active,
            "status should round-trip"
        );
        assert_eq!(parsed.tools.len(), 2, "tools should round-trip");
        assert!(parsed.archived.is_none(), "archived should be None");
    }

    #[test]
    fn frontmatter_with_archived_date() {
        let yaml = r#"
name: old-project
description: "Done"
status: archived
created: 2025-11-01
archived: 2026-02-08
"#;
        let fm: ProjectFrontmatter = serde_yml::from_str(yaml).unwrap();
        assert_eq!(fm.status, ProjectStatus::Archived, "should be archived");
        assert!(fm.archived.is_some(), "archived date should be present");
        assert_eq!(
            fm.archived.unwrap(),
            NaiveDate::from_ymd_opt(2026, 2, 8).unwrap(),
            "archived date should match"
        );
    }

    #[test]
    fn frontmatter_optional_fields_default() {
        let yaml = r#"
name: minimal
description: "Minimal project"
status: active
created: 2026-02-20
"#;
        let fm: ProjectFrontmatter = serde_yml::from_str(yaml).unwrap();
        assert!(fm.tools.is_empty(), "tools should default to empty");
        assert!(
            fm.mcp_servers.is_empty(),
            "mcp_servers should default to empty"
        );
        assert!(fm.archived.is_none(), "archived should default to None");
    }

    #[test]
    fn frontmatter_with_mcp_servers() {
        let yaml = r#"
name: with-mcp
description: "Has MCP servers"
status: active
created: 2026-02-20
mcp_servers:
  - filesystem
  - git
"#;
        let fm: ProjectFrontmatter = serde_yml::from_str(yaml).unwrap();
        assert_eq!(fm.mcp_servers.len(), 2, "should have two MCP server refs");
        assert_eq!(fm.mcp_servers[0], "filesystem", "first ref should match");
        assert_eq!(fm.mcp_servers[1], "git", "second ref should match");
    }

    #[test]
    fn mcp_transport_serde_round_trip_stdio() {
        let yaml = serde_yml::to_string(&McpTransport::Stdio).unwrap();
        assert!(yaml.contains("stdio"), "Stdio should serialize as 'stdio'");
        let parsed: McpTransport = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(parsed, McpTransport::Stdio, "should round-trip Stdio");
    }

    #[test]
    fn mcp_transport_serde_round_trip_http() {
        let yaml = serde_yml::to_string(&McpTransport::Http).unwrap();
        assert!(yaml.contains("http"), "Http should serialize as 'http'");
        let parsed: McpTransport = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(parsed, McpTransport::Http, "should round-trip Http");
    }

    #[test]
    fn mcp_transport_defaults_to_stdio() {
        assert_eq!(
            McpTransport::default(),
            McpTransport::Stdio,
            "default transport should be Stdio"
        );
    }

    #[test]
    fn frontmatter_with_mcp_server_references_round_trip() {
        let fm = ProjectFrontmatter {
            name: "ref-test".to_string(),
            description: "MCP ref round-trip".to_string(),
            status: ProjectStatus::Active,
            created: NaiveDate::from_ymd_opt(2026, 2, 20).unwrap(),
            tools: vec![],
            mcp_servers: vec!["filesystem".to_string(), "git".to_string()],
            archived: None,
        };

        let yaml = serde_yml::to_string(&fm).unwrap();
        let parsed: ProjectFrontmatter = serde_yml::from_str(&yaml).unwrap();

        assert_eq!(parsed.mcp_servers.len(), 2, "should round-trip two refs");
        assert_eq!(parsed.mcp_servers[0], "filesystem");
        assert_eq!(parsed.mcp_servers[1], "git");
    }

    #[test]
    fn status_serialization() {
        let active_yaml = serde_yml::to_string(&ProjectStatus::Active).unwrap();
        assert!(
            active_yaml.contains("active"),
            "Active should serialize as 'active'"
        );

        let archived_yaml = serde_yml::to_string(&ProjectStatus::Archived).unwrap();
        assert!(
            archived_yaml.contains("archived"),
            "Archived should serialize as 'archived'"
        );
    }

    #[test]
    fn status_display() {
        assert_eq!(
            ProjectStatus::Active.to_string(),
            "active",
            "Active display"
        );
        assert_eq!(
            ProjectStatus::Archived.to_string(),
            "archived",
            "Archived display"
        );
    }
}
