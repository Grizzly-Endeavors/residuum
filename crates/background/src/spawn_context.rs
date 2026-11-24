use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use anyhow::Context as _;

use crate::fork_failure::ForkFailure;
use crate::registry::SessionRegistry;
use crate::runtime::SessionRuntime;
use residuum_agent::context::loading::{load_observations, load_recent_context_narrative};
use residuum_agent::{HopCounter, TeamRoster};
use residuum_bus::session::SessionCategory;
use residuum_bus::{Publisher, SessionAddress};
use residuum_config::ProviderSpec;
use residuum_config::RetryConfig;
use residuum_config::{
    BackgroundConfig, BackgroundModelTier, ModelCatalog, ModelSelector, SharedModelCatalog,
    WorkspaceLayout,
};
use residuum_inference::{
    CompletionOptions, SharedHttpClient, build_provider_chain_with_shared_notices,
};
use residuum_mcp::SharedMcpRegistry;
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::Observer;
use residuum_skills::SharedSkillState;
use residuum_workspace::identity::IdentityFiles;

use super::messaging::AgentMessenger;
use super::session_tools::SessionToolFactory;
use super::subagent::{SubAgentResources, build_subagent_resources};
use super::types::SubAgentBuildConfig;

#[derive(Clone, Default)]
pub struct BackgroundTierActiveIndex {
    small: Arc<AtomicUsize>,
    medium: Arc<AtomicUsize>,
    large: Arc<AtomicUsize>,
}

impl BackgroundTierActiveIndex {
    fn for_tier(&self, tier: BackgroundModelTier) -> Arc<AtomicUsize> {
        match tier {
            BackgroundModelTier::Small => Arc::clone(&self.small),
            BackgroundModelTier::Medium => Arc::clone(&self.medium),
            BackgroundModelTier::Large => Arc::clone(&self.large),
        }
    }
}

pub struct SpawnContext {
    pub background_config: BackgroundConfig,
    pub main_provider_specs: Vec<ProviderSpec>,
    pub model_catalog: SharedModelCatalog,
    pub http_client: SharedHttpClient,
    pub max_tokens: u32,
    pub retry_config: RetryConfig,
    pub options: CompletionOptions,
    pub max_tool_iterations: Option<usize>,
    pub repeat_call_guard: residuum_config::RepeatCallGuardConfig,
    pub intermediate_delivery: residuum_agent::context::IntermediateDelivery,
    pub layout: WorkspaceLayout,
    pub role_overrides: std::collections::HashMap<String, residuum_config::RoleOverrides>,
    pub session_tools: Arc<dyn SessionToolFactory>,
    pub session_runtime: Arc<SessionRuntime>,
    pub session_registry: Arc<SessionRegistry>,
    pub publisher: Publisher,
    pub skill_state: SharedSkillState,
    pub mcp_registry: SharedMcpRegistry,
    pub observer: Arc<Observer>,
    pub merge_writer: Arc<MemoryMergeWriter>,
    pub messenger: Arc<AgentMessenger>,
    pub bg_tier_active_index: BackgroundTierActiveIndex,
    pub action_outcome: Option<ActionOutcomeSink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionSpawnOutcome {
    Started,
    Failed,
}

pub type ActionOutcomeSink =
    Arc<dyn Fn(&str, ActionSpawnOutcome) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub(crate) struct NewSessionContext {
    pub(crate) own_address: SessionAddress,
    pub(crate) own_depth: u32,
    pub(crate) category: SessionCategory,
    pub(crate) hop_count: u32,
    pub(crate) trigger: residuum_bus::EventTrigger,
    pub(crate) conversation_target: Option<residuum_bus::ConversationTarget>,
}

fn log_dropped_fallbacks(model: &ModelSelector, dropped: &[residuum_inference::DroppedFallback]) {
    for fallback in dropped {
        tracing::warn!(
            model = %model,
            provider = %fallback.name,
            error = %fallback.error,
            "dropped an unbuildable fallback provider for this background tier"
        );
    }
}

struct SessionModelChain {
    specs: Vec<ProviderSpec>,
    label: String,
    active_index: Arc<AtomicUsize>,
    overrides_key: Option<&'static str>,
}

impl SessionModelChain {
    fn first_model_id(&self) -> String {
        self.specs
            .first()
            .map(|spec| format!("{}/{}", spec.name, spec.model.model))
            .unwrap_or_default()
    }
}

fn resolve_model_chain(
    tiers: &residuum_config::BackgroundModelsConfig,
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
            let spec = catalog.resolve(id).map_err(|e| {
                anyhow::Error::msg(e).context(ForkFailure::UnknownModel { model: id.clone() })
            })?;
            Ok(SessionModelChain {
                specs: vec![spec],
                label: format!("background session ({id})"),
                active_index: Arc::new(AtomicUsize::new(0)),
                overrides_key: None,
            })
        }
    }
}

pub(crate) struct SpawnResources {
    pub(crate) resources: SubAgentResources,
    pub(crate) resolved_model: String,
}

async fn load_fork_identity_and_memory(
    layout: &WorkspaceLayout,
    team: &crate::team::TeamLink,
) -> Result<(IdentityFiles, Option<String>, Option<String>), anyhow::Error> {
    let identity = IdentityFiles::load(layout)
        .await
        .context(ForkFailure::IdentityFiles)?
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
    .with_context(|| ForkFailure::ProviderChain {
        model: model.to_string(),
    })?;
    log_dropped_fallbacks(model, &dropped);

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
        intermediate_delivery: ctx.intermediate_delivery,
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

        small_a.store(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(small_b.load(std::sync::atomic::Ordering::SeqCst), 1);

        assert_eq!(medium.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(large.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    fn spec(provider: &str, model: &str) -> ProviderSpec {
        residuum_config::ModelCatalog::default()
            .resolve(&format!("{provider}/{model}"))
            .unwrap()
    }

    #[test]
    fn a_tier_runs_its_failover_chain_with_its_overrides() {
        let tiers = residuum_config::BackgroundModelsConfig {
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
            &residuum_config::BackgroundModelsConfig::default(),
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
            &residuum_config::BackgroundModelsConfig::default(),
            &[spec("anthropic", "claude-sonnet-4-6")],
            &BackgroundTierActiveIndex::default(),
            &ModelCatalog::default(),
            &ModelSelector::Explicit("nowhere/x".to_string()),
        ) else {
            panic!("an unknown provider must not resolve");
        };
        let message = err.to_string();
        assert!(message.contains("nowhere/x"), "{message}");
        assert_eq!(
            err.downcast_ref::<ForkFailure>(),
            Some(&ForkFailure::UnknownModel {
                model: "nowhere/x".to_string()
            }),
            "the owner is told the model isn't available"
        );
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

        let (identity, _, _) =
            load_fork_identity_and_memory(&layout, &crate::team::TeamLink::alone("agent"))
                .await
                .unwrap();
        assert_eq!(identity.soul.as_deref(), Some("soul"));
        assert_eq!(identity.agents.as_deref(), Some("team rules"));
        assert_eq!(identity.user.as_deref(), Some("team user"));
        assert_eq!(identity.wiki_index.as_deref(), Some("team index"));
    }
}
