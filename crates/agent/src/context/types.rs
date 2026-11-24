use std::collections::BTreeSet;
use std::ops::Range;

pub struct MemoryContext<'a> {
    pub observations: Option<&'a str>,
    pub recent_context: Option<&'a str>,
}

#[derive(Default)]
pub struct SkillsContext<'a> {
    pub index: Option<&'a str>,
    pub active_instructions: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntermediateDelivery {
    pub telegram: bool,
    pub discord: bool,
    pub teams: bool,
}

impl IntermediateDelivery {
    #[must_use]
    pub fn from_config(cfg: &residuum_config::Config) -> Self {
        Self {
            telegram: cfg
                .telegram
                .as_ref()
                .is_some_and(|t| t.deliver_intermediate),
            discord: cfg.discord.as_ref().is_some_and(|d| d.deliver_intermediate),
            teams: cfg.teams.as_ref().is_some_and(|t| t.deliver_intermediate),
        }
    }
}

#[derive(Default)]
pub struct PromptContext<'a> {
    pub skills: SkillsContext<'a>,
    pub intermediate_delivery: IntermediateDelivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionKind {
    Identity,
    Harness,
    Memory,
    SkillsIndex,
    ActiveSkills,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RequestLayout {
    pub(crate) system_sections: Vec<(SectionKind, Range<usize>)>,
    pub(crate) status_lines: BTreeSet<usize>,
    pub(crate) builtin_tools: usize,
}
