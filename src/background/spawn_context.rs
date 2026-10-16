//! Spawn context: parameters needed to construct providers and `SubAgentResources`
//! for a new session run (pulse, action, webhook, or on-demand spawn).

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use anyhow::Context as _;

use crate::agent::HopCounter;
use crate::agent::context::loading::{load_observations, load_recent_context_narrative};
use crate::background::registry::SessionRegistry;
use crate::background::runtime::SessionRuntime;
use crate::bus::session::SessionCategory;
use crate::bus::{Publisher, SessionAddress};
use crate::config::ProviderSpec;
use crate::config::RetryConfig;
use crate::config::{
    BackgroundConfig, BackgroundModelTier, ModelCatalog, ModelSelector, SharedModelCatalog,
    WorkspaceLayout,
};
use crate::inference::{
    CompletionOptions, SharedHttpClient, build_provider_chain_with_shared_notices,
};
use crate::mcp::SharedMcpRegistry;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::Observer;
use crate::skills::SharedSkillState;
use crate::workspace::identity::IdentityFiles;

use super::messaging::AgentMessenger;
use super::session_tools::SessionToolFactory;
use super::subagent::{SubAgentResources, build_subagent_resources};
use super::types::SubAgentBuildConfig;

/// Shared failover-transition counters for the background model tiers.
///
/// A background tier's provider chain is rebuilt fresh for every session
/// spawn (see [`build_spawn_resources`]), unlike the main model's or the
/// subconscious's, which are built once and live until the next config
/// reload. Sharing one counter per tier here — held on `SpawnContext` for
/// that tier's whole lifetime, across every spawn that uses it — means a
/// transition notices exactly once when the tier actually fails over or
/// recovers, rather than once per spawn made while it's already degraded.
#[derive(Clone, Default)]
pub(crate) struct BackgroundTierActiveIndex {
    small: Arc<AtomicUsize>,
    medium: Arc<AtomicUsize>,
    large: Arc<AtomicUsize>,
}

impl BackgroundTierActiveIndex {
    /// This tier's shared counter, cloned (the `Arc`, not the count).
    fn for_tier(&self, tier: BackgroundModelTier) -> Arc<AtomicUsize> {
        match tier {
            BackgroundModelTier::Small => Arc::clone(&self.small),
            BackgroundModelTier::Medium => Arc::clone(&self.medium),
            BackgroundModelTier::Large => Arc::clone(&self.large),
        }
    }
}

/// Everything needed to fork a new session run from the gateway event loop.
pub(crate) struct SpawnContext {
    pub(crate) background_config: BackgroundConfig,
    pub(crate) main_provider_specs: Vec<ProviderSpec>,
    /// The live catalog an explicit model id resolves against at spawn time,
    /// shared with the tools that validate one, so a provider added to
    /// `providers.toml` works without a restart.
    pub(crate) model_catalog: SharedModelCatalog,
    pub(crate) http_client: SharedHttpClient,
    pub(crate) max_tokens: u32,
    pub(crate) retry_config: RetryConfig,
    pub(crate) options: CompletionOptions,
    /// Maximum tool-call iterations for a session turn before it stops
    /// itself gracefully, mirroring `cfg.agent.max_tool_iterations`. `None`
    /// means unlimited.
    pub(crate) max_tool_iterations: Option<usize>,
    /// Guards against a model repeating the exact same tool call, mirroring
    /// `cfg.agent.repeat_call_guard`.
    pub(crate) repeat_call_guard: crate::config::RepeatCallGuardConfig,
    pub(crate) layout: WorkspaceLayout,
    pub(crate) role_overrides: std::collections::HashMap<String, crate::config::RoleOverrides>,
    /// Builds every fork's tool registry, scoped to that session; it holds
    /// the shared handles only the session's tools read.
    pub(crate) session_tools: Arc<dyn SessionToolFactory>,
    pub(crate) session_runtime: Arc<SessionRuntime>,
    pub(crate) session_registry: Arc<SessionRegistry>,
    pub(crate) publisher: Publisher,
    /// Main agent skill state — cloned per fork into isolated session state.
    pub(crate) skill_state: SharedSkillState,
    /// Shared MCP registry (ref-counted across sessions).
    pub(crate) mcp_registry: SharedMcpRegistry,
    /// A session's own observer instance, built from the same `[observer]`
    /// config the main agent uses, for per-run threshold checks and
    /// extraction. Independent from the main agent's `Observer` instance so
    /// a session fork never contends with a main config-reload swap.
    pub(crate) observer: Arc<Observer>,
    /// The single serialized writer for global memory, shared with the main
    /// agent so episode numbering and log appends never race between a
    /// session's completion pipeline and the main agent's own observations.
    pub(crate) merge_writer: Arc<MemoryMergeWriter>,
    /// Shared agent-messaging service, whose team link gives every fork's
    /// identity its team roster.
    pub(crate) messenger: Arc<AgentMessenger>,
    /// Shared failover-transition counters for the background model tiers —
    /// see [`BackgroundTierActiveIndex`].
    pub(crate) bg_tier_active_index: BackgroundTierActiveIndex,
}

/// Identity and origin of the session [`build_spawn_resources`] is forking,
/// grouped into one struct to keep that function's argument count down.
pub(crate) struct NewSessionContext {
    /// This new session's own address, threaded into its `subagent_spawn`
    /// tool so any session it spawns in turn records the right spawner.
    pub(crate) own_address: SessionAddress,
    /// This new session's own depth from the main agent (main = 0).
    pub(crate) own_depth: u32,
    /// This session's category, carried into its tools' scope so its
    /// `message_agent` tool can report it.
    pub(crate) category: SessionCategory,
    /// Hop count of this session's first turn (see
    /// [`super::types::SubAgentConfig::hop_count`]).
    pub(crate) hop_count: u32,
    /// What triggered this session, carried into its tools' scope.
    pub(crate) trigger: crate::bus::EventTrigger,
    /// The conversation this session replies to, for a conversation-triggered
    /// session, carried into its tools' scope. `None` for every other
    /// trigger.
    pub(crate) conversation_target: Option<crate::bus::ConversationTarget>,
}

/// Log a warning for each fallback provider dropped from a background
/// tier's chain — visible in diagnostics without stopping the spawn, since
/// the tier's primary (or another fallback) still built successfully.
fn log_dropped_fallbacks(model: &ModelSelector, dropped: &[crate::inference::DroppedFallback]) {
    for fallback in dropped {
        tracing::warn!(
            model = %model,
            provider = %fallback.name,
            error = %fallback.error,
            "dropped an unbuildable fallback provider for this background tier"
        );
    }
}

/// A session's provider chain, resolved from its [`ModelSelector`].
struct SessionModelChain {
    specs: Vec<ProviderSpec>,
    /// Names the chain in failover notices.
    label: String,
    /// The chain's failover-transition counter.
    active_index: Arc<AtomicUsize>,
    /// The `role_overrides` key whose temperature/thinking apply, if any.
    overrides_key: Option<&'static str>,
}

impl SessionModelChain {
    /// `provider/model` id of the chain's first model, the one a run starts on.
    fn first_model_id(&self) -> String {
        self.specs
            .first()
            .map(|spec| format!("{}/{}", spec.name, spec.model.model))
            .unwrap_or_default()
    }
}

/// Resolve `model` to the provider chain a session runs on.
///
/// A tier resolves through `tiers` (falling back to the next tier up, then
/// `main`) and keeps its shared failover counter and its `bg_<tier>`
/// overrides. An explicit model resolves against `catalog` to that one model:
/// no failover chain, and no `role_overrides` entry, so the global
/// temperature/thinking apply.
///
/// # Errors
/// Returns an error naming the model if an explicit id doesn't resolve.
fn resolve_model_chain(
    tiers: &crate::config::BackgroundModelsConfig,
    main: &[ProviderSpec],
    tier_index: &BackgroundTierActiveIndex,
    catalog: &ModelCatalog,
    model: &ModelSelector,
) -> Result<SessionModelChain, anyhow::Error> {
    match model {
        ModelSelector::Tier(tier) => Ok(SessionModelChain {
            specs: tiers.resolve_tier(tier, main),
            label: format!("background sessions ({tier} tier)"),
            active_index: tier_index.for_tier(*tier),
            overrides_key: Some(match tier {
                BackgroundModelTier::Small => "bg_small",
                BackgroundModelTier::Medium => "bg_medium",
                BackgroundModelTier::Large => "bg_large",
            }),
        }),
        ModelSelector::Explicit(id) => {
            let spec = catalog
                .resolve(id)
                .map_err(|e| anyhow::anyhow!("model '{id}' can't be used: {e}"))?;
            Ok(SessionModelChain {
                specs: vec![spec],
                label: format!("background session ({id})"),
                active_index: Arc::new(AtomicUsize::new(0)),
                overrides_key: None,
            })
        }
    }
}

/// A forked session's resources, plus the model it starts on.
pub(crate) struct SpawnResources {
    pub(crate) resources: SubAgentResources,
    /// `provider/model` id of the first model in the session's chain.
    pub(crate) resolved_model: String,
}

/// Load identity fresh per fork and snapshot memory at fork time.
///
/// Identity (the agent's SOUL.md, the team's AGENTS.md/USER.md/wiki index) is
/// loaded fresh per fork so sessions see the current files; a read failure here fails the spawn — the caller
/// logs it loudly. The observation log and recent-context narrative are
/// snapshotted at fork time per the design's "Fork contents": a session
/// never sees merges that happen after it forked. A read failure snapshotting
/// either degrades gracefully (the run starts with no memory context) rather
/// than failing the spawn: an agent missing background is better than no
/// background work at all.
async fn load_fork_identity_and_memory(
    layout: &WorkspaceLayout,
    team: &crate::background::team::TeamLink,
) -> Result<(IdentityFiles, Option<String>, Option<String>), anyhow::Error> {
    let identity = IdentityFiles::load(layout)
        .await
        .context("failed to load identity files for session fork")?
        .with_team_roster(team.prompt_block());

    let observations = match load_observations(&layout.observations_json()).await {
        Ok(obs) => obs,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load observation snapshot for session fork");
            None
        }
    };
    let recent_context = match load_recent_context_narrative(&layout.recent_context_json()).await {
        Ok(narrative) => narrative,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent-context snapshot for session fork");
            None
        }
    };

    Ok((identity, observations, recent_context))
}

/// Build isolated `SubAgentResources` for a new session run on `model`.
///
/// Resolves the model (see [`resolve_model_chain`]), constructs the provider,
/// and builds fresh isolated state. When `skill` is set, that skill is activated
/// on the session's own skill state so its body becomes the session's role
/// instructions. Also snapshots the global observation log and recent-context
/// narrative at fork time, per the design's "Fork contents": a session never
/// sees merges that happen after it forked.
///
/// `session.own_address` and `session.own_depth` are this new session's own
/// address and depth, threaded into its `subagent_spawn` tool so any session
/// it spawns in turn records the right spawner and depth (see "Nesting" in
/// the design). `session.own_address` and `session.category` are also
/// carried into the session's tool scope so the fork's `message_agent` tool
/// can name itself as the sender of any message it sends. `session.trigger`
/// and `session.conversation_target` are carried the same way, so a tool
/// running in this session can tell what started it and, for a
/// conversation-triggered session, which endpoint and conversation it
/// replies to.
///
/// # Errors
/// Returns an error if an explicit model doesn't resolve, provider
/// construction fails (e.g. missing API key), the identity files cannot be
/// read, or `skill` names a skill that does not resolve.
#[tracing::instrument(skip_all, fields(model = %model, skill = skill.unwrap_or("none")))]
pub(crate) async fn build_spawn_resources(
    ctx: &SpawnContext,
    model: &ModelSelector,
    skill: Option<&str>,
    session: NewSessionContext,
) -> Result<SpawnResources, anyhow::Error> {
    let NewSessionContext {
        own_address,
        own_depth,
        category,
        hop_count,
        trigger,
        conversation_target,
    } = session;
    let chain = resolve_model_chain(
        &ctx.background_config.models,
        &ctx.main_provider_specs,
        &ctx.bg_tier_active_index,
        &*ctx.model_catalog.read().await,
        model,
    )?;
    let resolved_model = chain.first_model_id();

    let (provider, dropped) = build_provider_chain_with_shared_notices(
        &chain.specs,
        ctx.max_tokens,
        ctx.http_client.clone(),
        ctx.retry_config.clone(),
        Arc::new(ctx.publisher.clone()),
        chain.label,
        chain.active_index,
    )
    .with_context(|| format!("failed to build provider chain for model {model}"))?;
    log_dropped_fallbacks(model, &dropped);

    // Apply the tier's overrides over global options
    let ov = chain
        .overrides_key
        .and_then(|key| ctx.role_overrides.get(key));
    let options = CompletionOptions {
        max_tokens: Some(ctx.max_tokens),
        temperature: ov.and_then(|o| o.temperature).or(ctx.options.temperature),
        thinking: ov
            .and_then(|o| o.thinking.clone())
            .or(ctx.options.thinking.clone()),
        ..CompletionOptions::default()
    };

    let (identity, observations, recent_context) =
        load_fork_identity_and_memory(&ctx.layout, ctx.messenger.team()).await?;

    let build_config = SubAgentBuildConfig {
        workspace_layout: ctx.layout.clone(),
        identity,
        options,
        max_tool_iterations: ctx.max_tool_iterations,
        repeat_call_guard: ctx.repeat_call_guard,
        skill: skill.map(str::to_string),
        observations,
        recent_context,
        observer: Arc::clone(&ctx.observer),
        merge_writer: Arc::clone(&ctx.merge_writer),
        episode_skip_token_floor: ctx.background_config.episode_skip_token_floor,
        session_tools: Arc::clone(&ctx.session_tools),
        own_address,
        own_depth,
        session_category: category,
        trigger,
        conversation_target,
        hop_counter: HopCounter::new(hop_count),
    };

    let resources = build_subagent_resources(
        provider,
        &ctx.skill_state,
        Arc::clone(&ctx.mcp_registry),
        build_config,
    )
    .await?;
    Ok(SpawnResources {
        resources,
        resolved_model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tier_gets_its_own_stable_counter() {
        let counters = BackgroundTierActiveIndex::default();

        let small_a = counters.for_tier(BackgroundModelTier::Small);
        let small_b = counters.for_tier(BackgroundModelTier::Small);
        let medium = counters.for_tier(BackgroundModelTier::Medium);
        let large = counters.for_tier(BackgroundModelTier::Large);

        // Same tier, two lookups: the same underlying counter (an `Arc`
        // clone), so a store through one is visible through the other —
        // this is what lets many short-lived spawns for one tier share
        // transition state.
        small_a.store(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(small_b.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Different tiers never share a counter.
        assert_eq!(medium.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(large.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    fn spec(provider: &str, model: &str) -> ProviderSpec {
        crate::config::ModelCatalog::default()
            .resolve(&format!("{provider}/{model}"))
            .unwrap()
    }

    #[test]
    fn a_tier_runs_its_failover_chain_with_its_overrides() {
        let tiers = crate::config::BackgroundModelsConfig {
            small: Some(vec![
                spec("anthropic", "claude-haiku-4-5"),
                spec("openai", "gpt-4o-mini"),
            ]),
            medium: None,
            large: None,
        };
        let chain = resolve_model_chain(
            &tiers,
            &[spec("anthropic", "claude-sonnet-4-6")],
            &BackgroundTierActiveIndex::default(),
            &ModelCatalog::default(),
            &ModelSelector::SMALL,
        )
        .unwrap();
        assert_eq!(chain.specs.len(), 2, "the tier keeps its failover chain");
        assert_eq!(chain.overrides_key, Some("bg_small"));
    }

    #[test]
    fn an_explicit_model_runs_that_one_model_with_no_overrides() {
        let chain = resolve_model_chain(
            &crate::config::BackgroundModelsConfig::default(),
            &[spec("anthropic", "claude-sonnet-4-6")],
            &BackgroundTierActiveIndex::default(),
            &ModelCatalog::default(),
            &ModelSelector::Explicit("openai/gpt-4o".to_string()),
        )
        .unwrap();
        let [only] = chain.specs.as_slice() else {
            panic!("an explicit model has no failover chain: {:?}", chain.specs);
        };
        assert_eq!(only.model.model, "gpt-4o");
        assert_eq!(chain.overrides_key, None);
    }

    #[test]
    fn an_explicit_model_naming_no_provider_fails_the_spawn_naming_it() {
        let Err(err) = resolve_model_chain(
            &crate::config::BackgroundModelsConfig::default(),
            &[spec("anthropic", "claude-sonnet-4-6")],
            &BackgroundTierActiveIndex::default(),
            &ModelCatalog::default(),
            &ModelSelector::Explicit("nowhere/x".to_string()),
        ) else {
            panic!("an unknown provider must not resolve");
        };
        let message = err.to_string();
        assert!(message.contains("nowhere/x"), "{message}");
    }

    #[tokio::test]
    async fn fork_identity_reads_team_files() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("scout"));
        let team = layout.team();
        tokio::fs::create_dir_all(layout.root()).await.unwrap();
        tokio::fs::create_dir_all(team.wiki_dir()).await.unwrap();
        tokio::fs::write(layout.soul_md(), "soul").await.unwrap();
        tokio::fs::write(team.agents_md(), "team rules")
            .await
            .unwrap();
        tokio::fs::write(team.user_md(), "team user").await.unwrap();
        tokio::fs::write(team.wiki_index_md(), "team index")
            .await
            .unwrap();

        let (identity, _, _) = load_fork_identity_and_memory(
            &layout,
            &crate::background::team::TeamLink::alone("agent"),
        )
        .await
        .unwrap();
        assert_eq!(identity.soul.as_deref(), Some("soul"));
        assert_eq!(identity.agents.as_deref(), Some("team rules"));
        assert_eq!(identity.user.as_deref(), Some("team user"));
        assert_eq!(identity.wiki_index.as_deref(), Some("team index"));
    }
}
