//! The request a default main agent sends: golden files of its system prompt
//! and tool list, and checks that a reload that changes nothing leaves the
//! request's prefix byte-identical.
//!
//! The golden files live in `crates/tools/golden/`. A change to prompt or
//! tool text shows up as a diff to them; `just golden-update` rewrites them
//! from the current code, and the diff is reviewed with the change.
//!
//! The agent is a freshly bootstrapped workspace named `assistant` (its
//! first-run `SOUL.md` and `BOOTSTRAP.md`, the team's default `AGENTS.md`,
//! `USER.md` and wiki index, and the bundled skills), with the main tool set
//! built the way startup builds it (UTC, the default model catalog, every
//! optional tool turned on). Nothing in the system prompt or the tool list
//! depends on the clock; the hub's temporary directory is replaced by
//! `<hub>` so the files don't depend on where the test runs.
//!
//! `exec`'s description names the platform's shell, so the golden files are
//! checked on Unix only.

use residuum_agent::ToolRegistry;
use residuum_agent::testing::{RequestInputs, render_model_request};
use residuum_config::{SkillDir, WorkspaceLayout};
use residuum_skills::{SkillIndex, SkillState};
use residuum_workspace::identity::IdentityFiles;
use residuum_workspace::testing::TeamHub;

use crate::testing::RegistryHarness;

/// A bootstrapped workspace for the agent `assistant` in `hub`.
async fn bootstrapped_layout(hub: &TeamHub) -> WorkspaceLayout {
    let layout = WorkspaceLayout::new(hub.agent_dir("assistant"));
    residuum_workspace::team::ensure_team(layout.team(), None, None)
        .await
        .unwrap();
    residuum_workspace::bootstrap::ensure_workspace(&layout, &hub.coordinator, None, None)
        .await
        .unwrap();
    layout
}

/// The skill directories a default agent scans, in priority order.
fn skill_dirs(layout: &WorkspaceLayout) -> Vec<SkillDir> {
    vec![
        SkillDir::agent(layout.skills_dir()),
        SkillDir::team(layout.team().skills_dir()),
    ]
}

/// The golden files, checked on Unix only (see the module doc).
#[cfg(unix)]
mod golden {
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    use residuum_agent::ToolRegistry;
    use residuum_agent::testing::{RequestInputs, render_model_request};
    use residuum_config::WorkspaceLayout;
    use residuum_inference::ToolDefinition;
    use residuum_skills::SkillIndex;
    use residuum_workspace::identity::IdentityFiles;
    use residuum_workspace::testing::TeamHub;

    use super::{bootstrapped_layout, skill_dirs};
    use crate::testing::RegistryHarness;

    /// Set to rewrite the golden files instead of checking them.
    const UPDATE_VAR: &str = "RESIDUUM_UPDATE_GOLDEN";

    fn golden_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("golden")
            .join(name)
    }

    /// Check `actual` against the golden file `name`, or rewrite the file when
    /// [`UPDATE_VAR`] is set.
    fn assert_golden(name: &str, actual: &str) {
        let path = golden_path(name);
        if std::env::var_os(UPDATE_VAR).is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "can't read golden file {}: {e}; run `just golden-update` to create it",
                path.display()
            )
        });
        if expected == actual {
            return;
        }
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(want, got)| want != got)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} no longer matches what the code renders, from line {}:\n  golden: {:?}\n  now:    {:?}\n\
             If the change is intended, run `just golden-update` and review the diff to {}.",
            line + 1,
            expected.lines().nth(line).unwrap_or("<end of file>"),
            actual.lines().nth(line).unwrap_or("<end of file>"),
            path.display()
        );
    }

    /// The request inputs of a default agent's first turn.
    async fn default_agent_inputs(layout: &WorkspaceLayout) -> RequestInputs {
        let identity = IdentityFiles::load(layout).await.unwrap();
        let index = SkillIndex::scan(&skill_dirs(layout)).await.unwrap();
        RequestInputs {
            identity,
            skills_index: Some(index.format_for_prompt()),
            ..RequestInputs::default()
        }
    }

    /// Each tool as a heading, its description, and its parameter schema.
    fn render_tools(tools: &[ToolDefinition]) -> String {
        let mut out = String::new();
        for tool in tools {
            let schema = serde_json::to_string_pretty(&tool.parameters).unwrap();
            writeln!(
                out,
                "## {}\n\n{}\n\n{schema}\n",
                tool.name, tool.description
            )
            .unwrap();
        }
        out
    }

    /// `text` with the hub's temporary directory replaced by `<hub>`.
    fn without_hub_dir(text: &str, hub: &TeamHub) -> String {
        text.replace(&hub.dir.path().display().to_string(), "<hub>")
    }

    #[tokio::test]
    async fn the_default_system_prompt_matches_its_golden_file() {
        let hub = TeamHub::new();
        let layout = bootstrapped_layout(&hub).await;
        let request =
            render_model_request(&default_agent_inputs(&layout).await, &ToolRegistry::new()).await;

        let system = request.messages.first().unwrap();
        assert_golden(
            "default_system_prompt.txt",
            &without_hub_dir(&system.content, &hub),
        );
    }

    #[tokio::test]
    async fn the_default_tool_list_matches_its_golden_file() {
        let dir = tempfile::tempdir().unwrap();
        let harness = RegistryHarness::build(dir.path()).await;
        let request =
            render_model_request(&RequestInputs::default(), &harness.main_registry()).await;

        let rendered =
            render_tools(&request.tools).replace(&dir.path().display().to_string(), "<workspace>");
        assert_golden("default_tools.txt", &rendered);
    }
}

/// The tools and system prompt of the request `inputs` and `tools` render,
/// as the JSON they serialize to.
async fn sent_prefix(inputs: &RequestInputs, tools: &ToolRegistry) -> (String, String) {
    let request = render_model_request(inputs, tools).await;
    (
        serde_json::to_string(&request.tools).unwrap(),
        serde_json::to_string(request.messages.first().unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn a_config_reload_that_changes_nothing_keeps_the_request_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let harness = RegistryHarness::build(dir.path()).await;
    let mut tools = harness.main_registry();
    let inputs = RequestInputs::default();
    let before = sent_prefix(&inputs, &tools).await;

    // What the gateway's config reload does to the main registry.
    crate::registration::reload_timezone_tools(
        &mut tools,
        &crate::registration::RegistryInjections::default(),
        std::sync::Arc::clone(&harness.action_store),
        std::sync::Arc::clone(&harness.action_notify),
        residuum_config::ModelCatalog::default().into_shared(),
        &harness.layout,
        chrono_tz::UTC,
    );
    crate::registration::reload_ollama_web_search_tool(
        &mut tools,
        harness.cfg.web_search.standalone_backend.as_ref(),
    );

    assert_eq!(
        sent_prefix(&inputs, &tools).await,
        before,
        "an unchanged config re-registers its tools with the same bytes in the same places"
    );
}

#[tokio::test]
async fn a_skill_rescan_that_finds_nothing_new_keeps_the_request_prefix() {
    let hub = TeamHub::new();
    let layout = bootstrapped_layout(&hub).await;
    let index = SkillIndex::scan(&skill_dirs(&layout)).await.unwrap();
    let mut state = SkillState::new(index, skill_dirs(&layout));
    let active = state
        .index()
        .entries()
        .first()
        .map(|entry| entry.name.clone())
        .expect("the bundled skills are installed");
    state.activate(&active).await.unwrap();

    let identity = IdentityFiles::load(&layout).await.unwrap();
    let inputs_of = |state: &SkillState| RequestInputs {
        identity: identity.clone(),
        skills_index: Some(state.format_index_for_prompt()),
        active_skills: state.format_active_for_prompt(),
        ..RequestInputs::default()
    };
    let tools = ToolRegistry::new();
    let before = sent_prefix(&inputs_of(&state), &tools).await;

    // What the gateway's config reload does to the skills.
    state.rescan().await.unwrap();

    assert_eq!(
        sent_prefix(&inputs_of(&state), &tools).await,
        before,
        "a rescan of unchanged skill directories keeps the index and the active skill's text"
    );
}
