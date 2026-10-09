//! Workspace bootstrapping: creates required directories and default identity files.

use crate::util::FatalError;

use super::layout::WorkspaceLayout;
use super::team_files::TeamWriteCoordinator;

// ── Workspace bootstrap content (embedded at compile time from assets/) ──────

const DEFAULT_SOUL: &str = include_str!("../../assets/workspace-bootstrap/SOUL.md");

/// Default content for BOOTSTRAP.md -- first-run guidance.
///
/// This file is written once during workspace creation and should be deleted
/// by the agent after the first conversation. A `.bootstrapped` sentinel file
/// prevents it from being recreated on subsequent startups.
const DEFAULT_BOOTSTRAP: &str = include_str!("../../assets/workspace-bootstrap/BOOTSTRAP.md");

/// Default observer content guidance written to memory/OBSERVER.md.
///
/// Contains only the customizable content portion — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_OBSERVER_PROMPT: &str =
    include_str!("../../assets/workspace-bootstrap/memory/OBSERVER.md");

/// Default reflector content guidance written to memory/REFLECTOR.md.
///
/// Contains only the customizable content portion — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_REFLECTOR_PROMPT: &str =
    include_str!("../../assets/workspace-bootstrap/memory/REFLECTOR.md");

/// Default `config/agent-card.json` -- what this agent advertises to other
/// agents reaching it over A2A. See `docs/systems-usage/a2a.md`.
const DEFAULT_AGENT_CARD: &str =
    include_str!("../../assets/workspace-bootstrap/config/agent-card.json");

/// Built-in `introspection` skill, used by the reflection pulse to review
/// episode memory and deliver suggestions.
const INTROSPECTION_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/introspection/SKILL.md");

/// Built-in `learner` skill, spawned by the subconscious when a single learnable
/// signal is detected in the live conversation. Corroborates the signal and makes
/// it durable (preference promotion or a queued recovery fix).
const LEARNER_SKILL_MD: &str = include_str!("../../assets/bundled-skills/learner/SKILL.md");

/// Built-in `memory-analyst` skill, used to answer synthesized questions about the
/// user or past history so the main agent gets grounded conclusions instead of raw
/// search excerpts.
const MEMORY_ANALYST_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/memory-analyst/SKILL.md");

/// Built-in `wiki` skill: the knowledge wiki's page format, index rules, and
/// ingest/lint procedures. Activated before writing to the wiki, and the role
/// of the `memory_tending` and `wiki_lint` pulses.
const WIKI_SKILL_MD: &str = include_str!("../../assets/bundled-skills/wiki/SKILL.md");

/// Built-in `workbench` skill: building interactive HTML artifacts in `workbench/`
/// that open in a tab of their own, with the injected `residuum` SDK.
const WORKBENCH_SKILL_MD: &str = include_str!("../../assets/bundled-skills/workbench/SKILL.md");

/// Endpoint, event, and blocked-route reference for the `workbench` skill.
const WORKBENCH_REF_API: &str =
    include_str!("../../assets/bundled-skills/workbench/references/api.md");

/// Default subconscious check policy written to SUBCONSCIOUS.md.
///
/// Contains only the customizable check guidance — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_SUBCONSCIOUS: &str = include_str!("../../assets/workspace-bootstrap/SUBCONSCIOUS.md");

// ── Bundled skill content (embedded at compile time from assets/) ────────────

// residuum-system skill
const SYSTEM_SKILL_MD: &str = include_str!("../../assets/bundled-skills/residuum-system/SKILL.md");

/// Every reference file of the residuum-system skill, as `(file name, content)`.
/// SKILL.md links to each by name; the bootstrap writes all of them.
const SYSTEM_REFS: &[(&str, &str)] = &[
    (
        "config.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/config.md"),
    ),
    (
        "agent-keys.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/agent-keys.md"),
    ),
    (
        "memory-system.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/memory-system.md"),
    ),
    (
        "checkpoints.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/checkpoints.md"),
    ),
    (
        "heartbeats.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/heartbeats.md"),
    ),
    (
        "inbox.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/inbox.md"),
    ),
    (
        "scheduled-actions.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/scheduled-actions.md"),
    ),
    (
        "skills.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/skills.md"),
    ),
    (
        "tools.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/tools.md"),
    ),
    (
        "mcp.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/mcp.md"),
    ),
    (
        "notifications.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/notifications.md"),
    ),
    (
        "background-tasks.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/background-tasks.md"),
    ),
    (
        "subconscious.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/subconscious.md"),
    ),
    (
        "a2a.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/a2a.md"),
    ),
    (
        "team-files.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/team-files.md"),
    ),
];

// residuum-getting-started skill
const GETTING_STARTED_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/residuum-getting-started/SKILL.md");
const GETTING_STARTED_ORGANIZED: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/getting-organized.md"
);
const GETTING_STARTED_MONITORING: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/monitoring-setup.md"
);
const GETTING_STARTED_EXTENDING: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/extending-capabilities.md"
);
const GETTING_STARTED_UNDERSTANDING: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/understanding-the-agent.md"
);
const GETTING_STARTED_ALWAYS_ON: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/always-on-assistant.md"
);

// skill-authoring skill
const SKILL_AUTHORING_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/skill-authoring/SKILL.md");
const SKILL_AUTHORING_REF_STANDARDS: &str =
    include_str!("../../assets/bundled-skills/skill-authoring/references/authoring-standards.md");

// teams-setup skill
const TEAMS_SETUP_SKILL_MD: &str = include_str!("../../assets/bundled-skills/teams-setup/SKILL.md");
const TEAMS_SETUP_M365AGENTS_YML: &str =
    include_str!("../../assets/bundled-skills/teams-setup/templates/m365agents.yml");
const TEAMS_SETUP_MANIFEST_JSON: &str =
    include_str!("../../assets/bundled-skills/teams-setup/templates/appPackage/manifest.json");
const TEAMS_SETUP_ENV_RESIDUUM: &str =
    include_str!("../../assets/bundled-skills/teams-setup/templates/env/.env.residuum");
const TEAMS_SETUP_COLOR_PNG: &[u8] =
    include_bytes!("../../assets/bundled-skills/teams-setup/templates/appPackage/color.png");
const TEAMS_SETUP_OUTLINE_PNG: &[u8] =
    include_bytes!("../../assets/bundled-skills/teams-setup/templates/appPackage/outline.png");

/// Ensure the agent's workspace directory structure exists with default
/// identity files, and that the shared team layer it belongs to exists.
///
/// The agent directory gets `SOUL.md`, `HEARTBEAT.yml`, `SUBCONSCIOUS.md`,
/// the memory prompts, its A2A card, and (once) `BOOTSTRAP.md`. `SOUL.md`
/// is named with the directory name. The team directory (see
/// [`super::team::ensure_team`]) gets the shared `AGENTS.md`, `USER.md` and
/// wiki skeleton, and the agent gets a role page in the team wiki. When
/// `user_name` is provided and the team's `USER.md` does not yet exist, the
/// default content is personalised with the user's name; when `timezone` is
/// provided, it is included as well.
///
/// This is idempotent: existing files and directories are not modified,
/// except a `SOUL.md` that is still the bundled template. That file gets the
/// agent's name, and its identity section under the name is left empty.
///
/// `coordinator` is the team write coordinator for `layout.team()`; the role
/// page and the roster files are written under its locks.
///
/// # Errors
/// Returns `FatalError::Workspace` if directories cannot be created or
/// default files cannot be written.
#[tracing::instrument(skip_all, fields(workspace = %layout.root().display()))]
pub async fn ensure_workspace(
    layout: &WorkspaceLayout,
    coordinator: &TeamWriteCoordinator,
    user_name: Option<&str>,
    timezone: Option<&str>,
) -> Result<(), FatalError> {
    ensure_workspace_labeled(layout, coordinator, user_name, timezone, None).await
}

/// [`ensure_workspace`], with `label` as the name in `SOUL.md` and on the
/// role page when it differs from the directory name. `None` uses the
/// directory name. An empty `label` is treated as `None`.
///
/// # Errors
/// Returns `FatalError::Workspace` if a directory or file cannot be created.
pub async fn ensure_workspace_labeled(
    layout: &WorkspaceLayout,
    coordinator: &TeamWriteCoordinator,
    user_name: Option<&str>,
    timezone: Option<&str>,
    label: Option<&str>,
) -> Result<(), FatalError> {
    // Create all required directories
    for dir in layout.required_dirs() {
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            FatalError::Workspace(format!("failed to create directory {}: {e}", dir.display()))
        })?;
    }

    // Migration: Dual Inbox
    // Move any old flat `inbox/*.json` files to `inbox/agent/`
    let old_inbox = layout.root().join("inbox");
    if let Ok(mut entries) = tokio::fs::read_dir(&old_inbox).await {
        let agent_inbox = layout.agent_inbox_dir();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == "json")
                && let Some(name) = path.file_name()
            {
                let new_path = agent_inbox.join(name);
                if let Err(e) = tokio::fs::rename(&path, &new_path).await {
                    tracing::warn!(old = %path.display(), new = %new_path.display(), error = %e, "failed to migrate inbox item");
                } else {
                    tracing::info!(item = %name.to_string_lossy(), "migrated inbox item to agent inbox");
                }
            }
        }
    }

    let Some(dir_name) = layout.agent_name() else {
        return Err(FatalError::Workspace(format!(
            "workspace {} has no directory name to use as the agent's name",
            layout.root().display()
        )));
    };
    let soul_name = label.filter(|label| !label.is_empty()).unwrap_or(dir_name);
    write_soul(&layout.soul_md(), soul_name).await?;

    super::team::ensure_team(layout.team(), user_name, timezone).await?;
    let writer = super::team_files::TeamWriter::Agent(dir_name.to_string());
    super::team::ensure_agent_role_page_as(
        layout.team(),
        coordinator,
        &writer,
        dir_name,
        soul_name,
        None,
    )
    .await?;

    // BOOTSTRAP.md is first-run only: write it once, then drop a sentinel so it
    // is never recreated after the agent deletes it.
    let sentinel = layout.root().join(".bootstrapped");
    let fresh_bootstrap = !tokio::fs::try_exists(&sentinel).await.map_err(|e| {
        FatalError::Workspace(format!(
            "failed to check bootstrap sentinel {}: {e}",
            sentinel.display()
        ))
    })?;
    if fresh_bootstrap {
        write_if_missing(&layout.bootstrap_md(), DEFAULT_BOOTSTRAP).await?;
        // Create the sentinel after writing BOOTSTRAP.md so that if we crash
        // between writing and sentinel creation, the next startup will retry.
        tokio::fs::write(&sentinel, "").await.map_err(|e| {
            FatalError::Workspace(format!(
                "failed to write bootstrap sentinel {}: {e}",
                sentinel.display()
            ))
        })?;
        tracing::debug!(sentinel = %sentinel.display(), "bootstrap sentinel written");
    }

    write_if_missing(&layout.observer_md(), DEFAULT_OBSERVER_PROMPT).await?;
    write_if_missing(&layout.reflector_md(), DEFAULT_REFLECTOR_PROMPT).await?;
    write_if_missing(
        &layout.heartbeat_yml(),
        super::team::first_agent_heartbeat(),
    )
    .await?;
    write_if_missing(&layout.subconscious_md(), DEFAULT_SUBCONSCIOUS).await?;
    write_if_missing(&layout.agent_card_json(), DEFAULT_AGENT_CARD).await?;

    // Write bundled skills
    write_bundled_skills(layout).await?;

    tracing::info!(
        workspace = %layout.root().display(),
        fresh_bootstrap,
        "workspace ready"
    );

    Ok(())
}

/// Identity lines removed from a soul that is otherwise the bundled template,
/// so the section under the name stays empty.
const STOCK_ARCHETYPE_LINE: &str =
    "- **Archetype**: Personal agent — part assistant, part collaborator, part automation layer\n";
const STOCK_TONE_LINE: &str = "- **Tone**: Calm, confident, and wise. Ready to get shit done. Skip the bullet points, just talk.";

/// The default `SOUL.md` with `name` as the agent's name. The identity
/// section under the name is empty.
fn soul_named(name: &str) -> String {
    DEFAULT_SOUL.replace("**Name**: Ralph", &format!("**Name**: {name}"))
}

fn strip_stock_identity(content: &str) -> String {
    let archetype_crlf = STOCK_ARCHETYPE_LINE.replace('\n', "\r\n");
    let tone_crlf = format!("{STOCK_TONE_LINE}\r\n");
    let tone_lf = format!("{STOCK_TONE_LINE}\n");
    content
        .replace(&archetype_crlf, "")
        .replace(STOCK_ARCHETYPE_LINE, "")
        .replace(&tone_crlf, "")
        .replace(&tone_lf, "")
        .replace(STOCK_TONE_LINE, "")
}

/// The bundled soul named `name`, when `existing` is still that template.
///
/// The template name is replaced with `name`. Stock archetype and tone lines
/// are dropped. A name the file already carries, other than the template
/// name, is kept. An edited file is left alone (`None`).
fn settled_soul(existing: &str, name: &str) -> Option<String> {
    let stripped = strip_stock_identity(existing);
    let name_line = stripped
        .lines()
        .find(|line| line.starts_with("- **Name**: "))?;
    let normalized = stripped.replacen(name_line, "- **Name**: Ralph", 1);
    if normalized.replace("\r\n", "\n") != DEFAULT_SOUL.replace("\r\n", "\n") {
        return None;
    }
    let rewritten = if name_line == "- **Name**: Ralph" {
        let soul = soul_named(name);
        if existing.contains("\r\n") {
            soul.replace('\n', "\r\n").replace("\r\r\n", "\r\n")
        } else {
            soul.replace("\r\n", "\n")
        }
    } else if stripped != existing {
        stripped
    } else {
        return None;
    };
    (rewritten != existing).then_some(rewritten)
}

/// Write `SOUL.md` named `name`, or bring an unedited template up to date.
async fn write_soul(path: &std::path::Path, name: &str) -> Result<(), FatalError> {
    match tokio::fs::read_to_string(path).await {
        Ok(existing) => {
            if let Some(rewritten) = settled_soul(&existing, name) {
                tokio::fs::write(path, rewritten).await.map_err(|e| {
                    FatalError::Workspace(format!(
                        "failed to set the name in {}: {e}",
                        path.display()
                    ))
                })?;
                tracing::info!(
                    path = %path.display(),
                    name,
                    "set the agent's name in SOUL.md"
                );
            } else {
                tracing::trace!(path = %path.display(), "SOUL.md already edited, leaving it");
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::write(path, soul_named(name))
                .await
                .map_err(|err| {
                    FatalError::Workspace(format!(
                        "failed to write default {}: {err}",
                        path.display()
                    ))
                })?;
            tracing::debug!(path = %path.display(), "created default identity file");
            Ok(())
        }
        Err(e) => Err(FatalError::Workspace(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

/// The identity and prompt files of the blank agent template, as
/// `(path under layout, content)`: a `SOUL.md` naming the agent, the
/// created-agent `HEARTBEAT.yml`, `SUBCONSCIOUS.md`, the memory prompts, and
/// the A2A card. `BOOTSTRAP.md` is not part of it, since created agents never
/// run the first-run interview. `config/config.toml` and
/// `config/providers.toml` are not part of it either; the caller writes them.
#[must_use]
pub(crate) fn blank_agent_template(
    layout: &WorkspaceLayout,
    name: &str,
) -> Vec<(std::path::PathBuf, String)> {
    vec![
        (layout.soul_md(), soul_named(name)),
        (
            layout.heartbeat_yml(),
            super::team::created_agent_heartbeat().to_string(),
        ),
        (layout.subconscious_md(), DEFAULT_SUBCONSCIOUS.to_string()),
        (layout.observer_md(), DEFAULT_OBSERVER_PROMPT.to_string()),
        (layout.reflector_md(), DEFAULT_REFLECTOR_PROMPT.to_string()),
        (layout.agent_card_json(), DEFAULT_AGENT_CARD.to_string()),
    ]
}

/// Write bundled skill trees to the team skills directory.
///
/// Bundled skills are team skills: every agent finds them through the team
/// layer, and the agent's own `skills/` starts empty.
///
/// Each file is written with `write_if_missing`, so user edits are preserved
/// and files are only recreated if deleted.
async fn write_bundled_skills(layout: &WorkspaceLayout) -> Result<(), FatalError> {
    let skills_root = layout.team().skills_dir();
    // residuum-system skill
    let system_dir = skills_root.join("residuum-system");
    let system_refs = system_dir.join("references");
    tokio::fs::create_dir_all(&system_refs).await.map_err(|e| {
        FatalError::Workspace(format!(
            "failed to create skill directory {}: {e}",
            system_refs.display()
        ))
    })?;

    write_if_missing(&system_dir.join("SKILL.md"), SYSTEM_SKILL_MD).await?;
    for (file_name, content) in SYSTEM_REFS {
        write_if_missing(&system_refs.join(file_name), content).await?;
    }

    // Single-file skills: role skills spawned as sub-agents by pulses, the
    // subconscious, and the main agent, plus the wiki conventions skill (also a
    // pulse role). None carries references, so each is a lone SKILL.md.
    for (name, body) in [
        ("introspection", INTROSPECTION_SKILL_MD),
        ("learner", LEARNER_SKILL_MD),
        ("memory-analyst", MEMORY_ANALYST_SKILL_MD),
        ("wiki", WIKI_SKILL_MD),
    ] {
        let dir = skills_root.join(name);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create skill directory {}: {e}",
                dir.display()
            ))
        })?;
        write_if_missing(&dir.join("SKILL.md"), body).await?;
    }

    // residuum-getting-started skill
    let started_dir = skills_root.join("residuum-getting-started");
    let started_workflows = started_dir.join("workflows");
    tokio::fs::create_dir_all(&started_workflows)
        .await
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create skill directory {}: {e}",
                started_workflows.display()
            ))
        })?;

    let started_skill = started_dir.join("SKILL.md");
    write_if_missing(&started_skill, GETTING_STARTED_SKILL_MD).await?;
    refresh_getting_started_actions(&started_skill).await?;
    write_if_missing(
        &started_workflows.join("getting-organized.md"),
        GETTING_STARTED_ORGANIZED,
    )
    .await?;
    write_if_missing(
        &started_workflows.join("monitoring-setup.md"),
        GETTING_STARTED_MONITORING,
    )
    .await?;
    write_if_missing(
        &started_workflows.join("extending-capabilities.md"),
        GETTING_STARTED_EXTENDING,
    )
    .await?;
    write_if_missing(
        &started_workflows.join("understanding-the-agent.md"),
        GETTING_STARTED_UNDERSTANDING,
    )
    .await?;
    write_if_missing(
        &started_workflows.join("always-on-assistant.md"),
        GETTING_STARTED_ALWAYS_ON,
    )
    .await?;

    // skill-authoring skill
    let authoring_dir = skills_root.join("skill-authoring");
    let authoring_refs = authoring_dir.join("references");
    tokio::fs::create_dir_all(&authoring_refs)
        .await
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create skill directory {}: {e}",
                authoring_refs.display()
            ))
        })?;

    write_if_missing(&authoring_dir.join("SKILL.md"), SKILL_AUTHORING_SKILL_MD).await?;
    write_if_missing(
        &authoring_refs.join("authoring-standards.md"),
        SKILL_AUTHORING_REF_STANDARDS,
    )
    .await?;

    // workbench skill
    let workbench_dir = skills_root.join("workbench");
    let workbench_refs = workbench_dir.join("references");
    tokio::fs::create_dir_all(&workbench_refs)
        .await
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create skill directory {}: {e}",
                workbench_refs.display()
            ))
        })?;
    write_if_missing(&workbench_dir.join("SKILL.md"), WORKBENCH_SKILL_MD).await?;
    write_if_missing(&workbench_refs.join("api.md"), WORKBENCH_REF_API).await?;

    // teams-setup skill
    write_teams_setup_skill(&skills_root).await?;

    tracing::debug!(workspace = %layout.root().display(), "wrote bundled skills");

    Ok(())
}

/// Write the bundled `teams-setup` skill and its scaffolding templates.
async fn write_teams_setup_skill(skills_root: &std::path::Path) -> Result<(), FatalError> {
    let teams_dir = skills_root.join("teams-setup");
    let teams_templates = teams_dir.join("templates");
    let teams_pkg = teams_templates.join("appPackage");
    let teams_env = teams_templates.join("env");
    tokio::fs::create_dir_all(&teams_pkg).await.map_err(|e| {
        FatalError::Workspace(format!(
            "failed to create skill directory {}: {e}",
            teams_pkg.display()
        ))
    })?;
    tokio::fs::create_dir_all(&teams_env).await.map_err(|e| {
        FatalError::Workspace(format!(
            "failed to create skill directory {}: {e}",
            teams_env.display()
        ))
    })?;
    write_if_missing(&teams_dir.join("SKILL.md"), TEAMS_SETUP_SKILL_MD).await?;
    write_if_missing(
        &teams_templates.join("m365agents.yml"),
        TEAMS_SETUP_M365AGENTS_YML,
    )
    .await?;
    write_if_missing(&teams_pkg.join("manifest.json"), TEAMS_SETUP_MANIFEST_JSON).await?;
    write_if_missing(&teams_pkg.join("color.png"), TEAMS_SETUP_COLOR_PNG).await?;
    write_if_missing(&teams_pkg.join("outline.png"), TEAMS_SETUP_OUTLINE_PNG).await?;
    write_if_missing(&teams_env.join(".env.residuum"), TEAMS_SETUP_ENV_RESIDUUM).await?;
    crate::interfaces::teams::atk::repair_stale_teams_template_files(&teams_templates)
        .await
        .map_err(|e| FatalError::Workspace(e.to_string()))?;
    Ok(())
}

const TONE_LINE_ACTION: &str =
    "- Update the **Tone** line in `SOUL.md` to reflect their preference\n";
const NAME_LINE_ACTION: &str = "- If they gave you a name or asked you to change something about your personality, update `SOUL.md` accordingly\n";

/// Point a getting-started skill that still names a Tone line at the empty
/// identity section. An edited skill that no longer has those lines is left
/// as it is.
fn refreshed_getting_started(content: &str) -> Option<String> {
    let tone_crlf = TONE_LINE_ACTION.replace('\n', "\r\n");
    let name_crlf = NAME_LINE_ACTION.replace('\n', "\r\n");
    if !content.contains(TONE_LINE_ACTION)
        && !content.contains(&tone_crlf)
        && !content.contains(NAME_LINE_ACTION)
        && !content.contains(&name_crlf)
    {
        return None;
    }
    let updated = content
        .replace(
            &tone_crlf,
            "- Write how they want you to communicate into the Identity section of `SOUL.md`, under your name. That section starts with your name and nothing else.\r\n",
        )
        .replace(
            TONE_LINE_ACTION,
            "- Write how they want you to communicate into the Identity section of `SOUL.md`, under your name. That section starts with your name and nothing else.\n",
        )
        .replace(
            &name_crlf,
            "- If they gave you a different name, or asked you to change something else about how you are, update `SOUL.md` accordingly\r\n",
        )
        .replace(
            NAME_LINE_ACTION,
            "- If they gave you a different name, or asked you to change something else about how you are, update `SOUL.md` accordingly\n",
        );
    (updated != content).then_some(updated)
}

async fn refresh_getting_started_actions(path: &std::path::Path) -> Result<(), FatalError> {
    let existing = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(FatalError::Workspace(format!(
                "failed to read {}: {error}",
                path.display()
            )));
        }
    };
    let Some(updated) = refreshed_getting_started(&existing) else {
        return Ok(());
    };
    tokio::fs::write(path, updated).await.map_err(|error| {
        FatalError::Workspace(format!(
            "failed to update the getting-started skill at {}: {error}",
            path.display()
        ))
    })?;
    tracing::info!(
        path = %path.display(),
        "pointed the getting-started skill at the empty identity section"
    );
    Ok(())
}

/// Write content to a file only if it does not already exist.
async fn write_if_missing(
    path: &std::path::Path,
    content: impl AsRef<[u8]>,
) -> Result<(), FatalError> {
    if tokio::fs::try_exists(path)
        .await
        .map_err(|e| FatalError::Workspace(format!("failed to check {}: {e}", path.display())))?
    {
        tracing::trace!(path = %path.display(), "identity file already exists, skipping");
    } else {
        tokio::fs::write(path, content.as_ref())
            .await
            .map_err(|e| {
                FatalError::Workspace(format!("failed to write default {}: {e}", path.display()))
            })?;
        tracing::debug!(path = %path.display(), "created default identity file");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_TEAM_USER: &str = include_str!("../../assets/team-bootstrap/USER.md");

    #[test]
    fn blank_template_names_the_agent_and_has_no_bootstrap_file() {
        let layout = WorkspaceLayout::new(std::path::Path::new("res").join("scout"));
        let files = blank_agent_template(&layout, "scout");

        let soul = files
            .iter()
            .find(|(path, _)| *path == layout.soul_md())
            .map(|(_, content)| content.as_str())
            .unwrap();
        assert!(soul.contains("**Name**: scout"));
        assert!(!soul.contains("Ralph"));
        assert!(!soul.contains("Archetype"));
        assert!(!soul.contains("**Tone**"));
        assert!(
            files.iter().all(|(path, _)| *path != layout.bootstrap_md()),
            "created agents never get BOOTSTRAP.md"
        );
    }

    #[tokio::test]
    async fn stale_team_teams_templates_are_repaired() {
        use crate::interfaces::teams::atk::STALE_TEAMS_TEMPLATE_TEXT;
        use crate::interfaces::teams::atk::tests::{
            bundled_teams_template, stale_bundled_teams_template,
        };
        let dir = tempfile::tempdir().unwrap();
        let templates = dir.path().join("teams-setup/templates");
        tokio::fs::create_dir_all(templates.join("appPackage"))
            .await
            .unwrap();
        for entry in STALE_TEAMS_TEMPLATE_TEXT {
            tokio::fs::write(
                templates.join(entry.file),
                stale_bundled_teams_template(entry),
            )
            .await
            .unwrap();
        }

        write_teams_setup_skill(dir.path()).await.unwrap();

        for entry in STALE_TEAMS_TEMPLATE_TEXT {
            assert_eq!(
                tokio::fs::read_to_string(templates.join(entry.file))
                    .await
                    .unwrap(),
                bundled_teams_template(entry.file)
            );
        }
    }

    #[test]
    fn a_getting_started_skill_that_names_a_tone_line_is_pointed_at_the_identity_section() {
        let old = format!("before\n{TONE_LINE_ACTION}{NAME_LINE_ACTION}after\n");
        let updated = refreshed_getting_started(&old).unwrap();
        assert!(updated.contains("under your name"));
        assert!(!updated.contains("**Tone** line"));
        assert!(updated.contains("a different name"));
        assert!(refreshed_getting_started(&updated).is_none());
        assert!(refreshed_getting_started("custom skill").is_none());
    }

    fn stock_soul() -> String {
        let (needle, archetype) = if DEFAULT_SOUL.contains("\r\n") {
            (
                "- **Name**: Ralph\r\n",
                STOCK_ARCHETYPE_LINE.replace('\n', "\r\n"),
            )
        } else {
            ("- **Name**: Ralph\n", STOCK_ARCHETYPE_LINE.to_string())
        };
        DEFAULT_SOUL.replacen(needle, &format!("{needle}{archetype}{STOCK_TONE_LINE}"), 1)
    }

    #[test]
    fn an_unedited_template_soul_takes_the_given_name_and_drops_the_stock_lines() {
        let settled = settled_soul(&stock_soul(), "Mist").unwrap();
        assert!(settled.contains("**Name**: Mist"));
        assert!(!settled.contains("Ralph"));
        assert!(!settled.contains("Archetype"));
        assert!(!settled.contains("**Tone**"));
        assert!(settled_soul(&settled, "Mist").is_none());
    }

    #[test]
    fn an_edited_soul_is_left_alone_even_when_its_name_is_still_the_template() {
        let edited = stock_soul().replace("Have opinions.", "Have few opinions.");
        assert!(settled_soul(&edited, "Mist").is_none());
    }

    #[test]
    fn a_soul_that_already_has_its_own_name_keeps_it_when_the_stock_lines_go() {
        let named = stock_soul().replace("**Name**: Ralph", "**Name**: Bob");
        let settled = settled_soul(&named, "Mist").unwrap();
        assert!(settled.contains("**Name**: Bob"));
        assert!(!settled.contains("Archetype"));
        assert!(settled_soul(&settled, "Mist").is_none());
    }

    #[test]
    fn a_soul_with_crlf_line_endings_settles_properly() {
        let crlf_stock = stock_soul().replace('\n', "\r\n").replace("\r\r\n", "\r\n");
        let named = crlf_stock.replace("**Name**: Ralph", "**Name**: Bob");
        let settled = settled_soul(&named, "Mist").unwrap();
        assert!(settled.contains("**Name**: Bob"));
        assert!(!settled.contains("Archetype"));
        assert!(settled_soul(&settled, "Mist").is_none());
    }

    #[tokio::test]
    async fn bootstrap_writes_the_given_name_into_an_empty_identity_section() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("mist"));

        ensure_workspace_labeled(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
            Some("Mist"),
        )
        .await
        .unwrap();

        let soul = tokio::fs::read_to_string(layout.soul_md()).await.unwrap();
        assert!(soul.contains("**Name**: Mist"), "{soul}");
        assert!(!soul.contains("Ralph"));
        assert!(!soul.contains("Archetype"));
        assert!(!soul.contains("**Tone**"));
    }

    #[tokio::test]
    async fn bootstrap_replaces_an_unedited_template_soul() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("mist"));
        let coordinator = crate::workspace::team_files::TeamWriteCoordinator::new(layout.team());

        ensure_workspace_labeled(&layout, &coordinator, None, None, Some("Mist"))
            .await
            .unwrap();
        tokio::fs::write(layout.soul_md(), stock_soul())
            .await
            .unwrap();

        ensure_workspace_labeled(&layout, &coordinator, None, None, Some("Mist"))
            .await
            .unwrap();

        let soul = tokio::fs::read_to_string(layout.soul_md()).await.unwrap();
        assert!(soul.contains("**Name**: Mist"), "{soul}");
        assert!(!soul.contains("Archetype"), "{soul}");
        assert!(!soul.contains("**Tone**"), "{soul}");
    }

    #[tokio::test]
    async fn bootstrap_creates_structure() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        assert!(layout.root().exists(), "root should exist");
        assert!(layout.memory_dir().exists(), "memory dir should exist");
        assert!(layout.episodes_dir().exists(), "episodes dir should exist");
        assert!(layout.skills_dir().exists(), "skills dir should exist");
        assert!(layout.soul_md().exists(), "SOUL.md should exist");
        let team = layout.team();
        assert!(team.agents_md().exists(), "team AGENTS.md should exist");
        assert!(team.user_md().exists(), "team USER.md should exist");
        assert!(
            team.wiki_index_md().exists(),
            "team wiki/index.md should exist"
        );
        assert!(
            team.agent_role_page("workspace").exists(),
            "the agent's role page should exist"
        );
        assert!(
            !layout.root().join("AGENTS.md").exists(),
            "AGENTS.md belongs to the team"
        );
        assert!(
            !layout.root().join("USER.md").exists(),
            "USER.md belongs to the team"
        );
        assert!(
            !layout.root().join("wiki").exists(),
            "the wiki belongs to the team"
        );
        assert!(layout.bootstrap_md().exists(), "BOOTSTRAP.md should exist");
        assert!(layout.observer_md().exists(), "OBSERVER.md should exist");
        assert!(layout.reflector_md().exists(), "REFLECTOR.md should exist");
        assert!(
            layout.heartbeat_yml().exists(),
            "HEARTBEAT.yml should exist"
        );
        assert!(
            layout.subconscious_md().exists(),
            "SUBCONSCIOUS.md should exist"
        );
        assert!(
            layout.agent_card_json().exists(),
            "config/agent-card.json should exist"
        );
        assert!(layout.agent_inbox_dir().exists(), "inbox dir should exist");
        assert!(
            layout.agent_inbox_archive_dir().exists(),
            "inbox archive dir should exist"
        );

        let soul = tokio::fs::read_to_string(layout.soul_md()).await.unwrap();
        assert!(!soul.is_empty(), "SOUL.md should have default content");
        let user = tokio::fs::read_to_string(layout.team().user_md())
            .await
            .unwrap();
        assert!(!user.is_empty(), "USER.md should have default content");
    }

    #[tokio::test]
    async fn bootstrap_creates_role_skills() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        for name in ["introspection", "learner", "memory-analyst", "wiki"] {
            let skill_path = layout.team().skills_dir().join(name).join("SKILL.md");
            assert!(skill_path.exists(), "{name}/SKILL.md should be created");

            let content = tokio::fs::read_to_string(&skill_path).await.unwrap();
            assert!(
                content.contains(&format!("name: {name}")),
                "{name}/SKILL.md should carry its own frontmatter name"
            );
        }
    }

    #[tokio::test]
    async fn bundled_skills_land_in_team_and_agent_skills_start_empty() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        assert!(
            layout
                .team()
                .skills_dir()
                .join("residuum-system")
                .join("SKILL.md")
                .exists(),
            "bundled skills belong to the team layer"
        );
        let mut agent_skills = tokio::fs::read_dir(layout.skills_dir()).await.unwrap();
        assert!(
            agent_skills.next_entry().await.unwrap().is_none(),
            "a fresh agent's own skills/ starts empty"
        );
    }

    #[tokio::test]
    async fn bootstrap_does_not_overwrite_existing_role_skill() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        let skill_path = layout
            .team()
            .skills_dir()
            .join("introspection")
            .join("SKILL.md");
        tokio::fs::write(&skill_path, "user-edited skill")
            .await
            .unwrap();

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(&skill_path).await.unwrap();
        assert_eq!(
            content, "user-edited skill",
            "a second bootstrap must not clobber a user-edited skill"
        );
    }

    #[tokio::test]
    async fn bootstrap_creates_bundled_skills() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        // residuum-system skill tree
        let system_dir = layout.team().skills_dir().join("residuum-system");
        assert!(system_dir.join("SKILL.md").exists(), "system SKILL.md");
        for (file_name, _) in SYSTEM_REFS {
            assert!(
                system_dir.join("references").join(file_name).exists(),
                "residuum-system reference {file_name} should be written"
            );
        }

        // residuum-getting-started skill tree
        let started_dir = layout.team().skills_dir().join("residuum-getting-started");
        assert!(
            started_dir.join("SKILL.md").exists(),
            "getting-started SKILL.md"
        );
        assert!(
            started_dir.join("workflows/getting-organized.md").exists(),
            "getting-organized.md"
        );
        assert!(
            started_dir.join("workflows/monitoring-setup.md").exists(),
            "monitoring-setup.md"
        );
        assert!(
            started_dir
                .join("workflows/extending-capabilities.md")
                .exists(),
            "extending-capabilities.md"
        );
        assert!(
            started_dir
                .join("workflows/understanding-the-agent.md")
                .exists(),
            "understanding-the-agent.md"
        );
        assert!(
            started_dir
                .join("workflows/always-on-assistant.md")
                .exists(),
            "always-on-assistant.md"
        );

        // skill-authoring skill tree
        let authoring_dir = layout.team().skills_dir().join("skill-authoring");
        assert!(
            authoring_dir.join("SKILL.md").exists(),
            "skill-authoring SKILL.md"
        );
        assert!(
            authoring_dir
                .join("references/authoring-standards.md")
                .exists(),
            "authoring-standards.md"
        );

        // workbench skill tree
        let workbench_dir = layout.team().skills_dir().join("workbench");
        let workbench_skill = tokio::fs::read_to_string(workbench_dir.join("SKILL.md"))
            .await
            .unwrap();
        assert!(
            workbench_skill.contains("name: workbench"),
            "workbench SKILL.md carries its frontmatter name"
        );
        assert!(
            workbench_dir.join("references/api.md").exists(),
            "workbench api.md"
        );

        let system_skill_content = tokio::fs::read_to_string(system_dir.join("SKILL.md"))
            .await
            .unwrap();
        assert!(
            !system_skill_content.is_empty(),
            "system SKILL.md should have content"
        );
    }

    #[test]
    fn system_skill_links_match_bundled_references() {
        let linked: std::collections::BTreeSet<&str> = SYSTEM_SKILL_MD
            .split("(references/")
            .skip(1)
            .filter_map(|rest| rest.split_once(')').map(|(name, _)| name))
            .collect();
        let bundled: std::collections::BTreeSet<&str> =
            SYSTEM_REFS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            linked, bundled,
            "every reference linked from residuum-system SKILL.md must be bundled, and every bundled reference linked"
        );
    }

    #[tokio::test]
    async fn bootstrap_seeds_team_wiki() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        let index = tokio::fs::read_to_string(layout.team().wiki_index_md())
            .await
            .unwrap();
        assert!(
            index.contains("okf_version"),
            "root wiki index should declare its OKF version"
        );
        assert!(
            layout.team().wiki_log_md().exists(),
            "team wiki/log.md should exist"
        );
        assert!(
            layout.team().skills_dir().join("wiki/SKILL.md").exists(),
            "wiki skill should be bundled"
        );
    }

    #[tokio::test]
    async fn bootstrap_writes_team_defaults_only_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        let team = layout.team();
        tokio::fs::create_dir_all(team.root()).await.unwrap();
        tokio::fs::write(team.agents_md(), "shared rules")
            .await
            .unwrap();

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            Some("Alex"),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(team.agents_md()).await.unwrap(),
            "shared rules",
            "an existing team AGENTS.md is kept"
        );
        assert!(team.user_md().exists(), "missing team files are written");
    }

    #[tokio::test]
    async fn bootstrap_gives_the_first_agent_the_wiki_lint_pulse() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        let heartbeat = tokio::fs::read_to_string(layout.heartbeat_yml())
            .await
            .unwrap();
        assert!(heartbeat.contains("name: memory_tending"));
        assert!(heartbeat.contains("name: wiki_lint"));
    }

    #[tokio::test]
    async fn bootstrap_recreates_a_missing_role_page_without_touching_the_agent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();
        tokio::fs::remove_file(layout.team().agent_role_page("workspace"))
            .await
            .unwrap();

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        assert!(layout.team().agent_role_page("workspace").exists());
    }

    #[tokio::test]
    async fn bootstrap_default_agent_card_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        let card = crate::a2a::AgentCardFile::load(&layout.agent_card_json()).unwrap();
        assert!(!card.name.trim().is_empty());
        assert!(!card.description.trim().is_empty());
        assert!(card.skills.is_empty(), "default card ships no skills");
    }

    #[tokio::test]
    async fn bootstrap_does_not_recreate_bootstrap_md_after_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        // First run: BOOTSTRAP.md and sentinel are created
        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            layout.bootstrap_md().exists(),
            "BOOTSTRAP.md should exist on first run"
        );
        assert!(
            layout.root().join(".bootstrapped").exists(),
            "sentinel should exist after first run"
        );

        // Simulate agent deleting BOOTSTRAP.md after first conversation
        tokio::fs::remove_file(layout.bootstrap_md()).await.unwrap();
        assert!(
            !layout.bootstrap_md().exists(),
            "BOOTSTRAP.md should be deleted"
        );

        // Second run: BOOTSTRAP.md should NOT be recreated
        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            !layout.bootstrap_md().exists(),
            "BOOTSTRAP.md should not be recreated after sentinel exists"
        );
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        // Modify SOUL.md
        tokio::fs::write(layout.soul_md(), "custom soul content")
            .await
            .unwrap();

        // Modify a skill file
        let system_skill = layout
            .team()
            .skills_dir()
            .join("residuum-system")
            .join("SKILL.md");
        tokio::fs::write(&system_skill, "user-edited skill")
            .await
            .unwrap();

        // Run again
        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        // Custom content should be preserved
        let content = tokio::fs::read_to_string(layout.soul_md()).await.unwrap();
        assert_eq!(
            content, "custom soul content",
            "existing files should not be overwritten"
        );

        let skill_content = tokio::fs::read_to_string(&system_skill).await.unwrap();
        assert_eq!(
            skill_content, "user-edited skill",
            "existing skill files should not be overwritten"
        );
    }

    #[tokio::test]
    async fn bootstrap_personalises_user_md_with_name() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            Some("Alex"),
            None,
        )
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(layout.team().user_md())
            .await
            .unwrap();
        assert!(
            content.contains("**Name**: Alex"),
            "USER.md should contain the user's name"
        );
    }

    #[tokio::test]
    async fn bootstrap_personalises_user_md_with_timezone() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            Some("Alex"),
            Some("America/New_York"),
        )
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(layout.team().user_md())
            .await
            .unwrap();
        assert!(
            content.contains("**Name**: Alex"),
            "USER.md should contain the user's name"
        );
        assert!(
            content.contains("**Timezone**: America/New_York"),
            "USER.md should contain the timezone"
        );
    }

    #[tokio::test]
    async fn bootstrap_default_user_md_without_name() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(layout.team().user_md())
            .await
            .unwrap();
        assert_eq!(
            content, DEFAULT_TEAM_USER,
            "USER.md should use default content when no name is provided"
        );
    }

    #[tokio::test]
    async fn bootstrap_timezone_only_user_md() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            Some("America/New_York"),
        )
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(layout.team().user_md())
            .await
            .unwrap();
        assert!(content.contains("**Timezone**: America/New_York"));
        assert!(!content.contains("**Name**"));
    }

    #[tokio::test]
    async fn bootstrap_empty_string_inputs_treated_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            Some(""),
            Some(""),
        )
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(layout.team().user_md())
            .await
            .unwrap();
        assert_eq!(
            content, DEFAULT_TEAM_USER,
            "empty strings should produce default USER.md"
        );
    }

    #[tokio::test]
    async fn teams_setup_bundled_skill_written_with_valid_templates_and_binary_icons() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(
            &layout,
            &crate::workspace::team_files::TeamWriteCoordinator::new(layout.team()),
            None,
            None,
        )
        .await
        .unwrap();

        let teams_dir = layout.team().skills_dir().join("teams-setup");
        let skill_md = tokio::fs::read_to_string(teams_dir.join("SKILL.md"))
            .await
            .unwrap();
        assert!(skill_md.contains("name: teams-setup"));
        assert!(skill_md.contains("Microsoft Teams"));

        // Verify m365agents.yml exists and parses as valid YAML
        let yml_str = tokio::fs::read_to_string(teams_dir.join("templates/m365agents.yml"))
            .await
            .unwrap();
        let yml_val: serde_json::Value = serde_yaml_ng::from_str(&yml_str).unwrap();
        assert_eq!(
            yml_val.get("version").and_then(|v| v.as_str()),
            Some("v1.13")
        );
        assert!(
            yml_val
                .get("provision")
                .is_some_and(serde_json::Value::is_array)
        );

        // Verify manifest.json exists and parses as valid JSON
        let manifest_str =
            tokio::fs::read_to_string(teams_dir.join("templates/appPackage/manifest.json"))
                .await
                .unwrap();
        let manifest_val: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();
        assert_eq!(
            manifest_val.get("manifestVersion").and_then(|v| v.as_str()),
            Some("1.17")
        );
        assert_eq!(
            manifest_val
                .get("icons")
                .and_then(|i| i.get("color"))
                .and_then(|v| v.as_str()),
            Some("color.png")
        );
        assert_eq!(
            manifest_val
                .get("icons")
                .and_then(|i| i.get("outline"))
                .and_then(|v| v.as_str()),
            Some("outline.png")
        );

        // Verify binary icons are valid PNGs matching embedded byte slices
        let color_png = tokio::fs::read(teams_dir.join("templates/appPackage/color.png"))
            .await
            .unwrap();
        assert_eq!(color_png, TEAMS_SETUP_COLOR_PNG);
        assert!(color_png.starts_with(b"\x89PNG\r\n\x1a\n"));

        let outline_png = tokio::fs::read(teams_dir.join("templates/appPackage/outline.png"))
            .await
            .unwrap();
        assert_eq!(outline_png, TEAMS_SETUP_OUTLINE_PNG);
        assert!(outline_png.starts_with(b"\x89PNG\r\n\x1a\n"));

        // Verify .env.residuum
        let env_str = tokio::fs::read_to_string(teams_dir.join("templates/env/.env.residuum"))
            .await
            .unwrap();
        assert!(env_str.contains("TEAMSFX_ENV=residuum"));
    }
}
