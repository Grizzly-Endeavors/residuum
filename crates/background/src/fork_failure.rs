//! Why a session fork failed, and how to tell the owner.
//!
//! The steps of a fork that fail for a reason the owner can act on attach a
//! [`ForkFailure`] as `anyhow` context where they fail. The listener reads it
//! back with a downcast to word a plain-language sentence, so the sentence
//! never depends on the wording of an error message.

use std::fmt;

use residuum_bus::SessionAddress;

/// The step of a session fork that failed.
///
/// Its `Display` is the developer wording that heads the error's cause chain
/// in logs and in a failure's details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForkFailure {
    /// An explicit `provider/model` id didn't resolve against the model
    /// catalog.
    UnknownModel {
        /// The model id the session asked for.
        model: String,
    },
    /// The provider chain for the session's model couldn't be built, most
    /// often because a provider's API key is missing.
    ProviderChain {
        /// The session's model: a tier name or a `provider/model` id.
        model: String,
    },
    /// The agent's or the team's identity files couldn't be read.
    IdentityFiles,
    /// The skill the session runs couldn't be activated.
    Skill {
        /// The skill's name.
        name: String,
    },
}

impl fmt::Display for ForkFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownModel { model } => write!(f, "model '{model}' can't be used"),
            Self::ProviderChain { model } => {
                write!(f, "failed to build provider chain for model {model}")
            }
            Self::IdentityFiles => f.write_str("failed to load identity files for session fork"),
            Self::Skill { name } => write!(f, "failed to activate skill '{name}' for sub-agent"),
        }
    }
}

/// The sentence that tells the owner the session they started at `address`
/// couldn't start, or, when `resume` is set, couldn't pick up the message
/// that resumed it: what went wrong and what to do next, from the
/// [`ForkFailure`] anywhere in `error`'s context chain.
pub(crate) fn owner_start_failure_message(
    address: &SessionAddress,
    resume: bool,
    error: &anyhow::Error,
) -> String {
    let (failed, again) = if resume {
        ("couldn't pick up your message", "send your message again")
    } else {
        ("couldn't start", "start the session again")
    };
    match error.downcast_ref::<ForkFailure>() {
        Some(ForkFailure::UnknownModel { model }) => {
            let next = if resume {
                "Start a new session on another model and send your message there."
            } else {
                "Choose another model and start the session again."
            };
            format!(
                "The session {address} {failed} because the model {model} isn't available. {next}"
            )
        }
        Some(ForkFailure::ProviderChain { model }) => format!(
            "The session {address} {failed} because its model ({model}) couldn't be set up, \
             usually because a provider's API key is missing. Check Settings → Model → \
             Providers, then {again}."
        ),
        Some(ForkFailure::IdentityFiles) => format!(
            "The session {address} {failed} because the agent's identity files couldn't be \
             read. Check that files such as SOUL.md exist and can be opened, then {again}."
        ),
        Some(ForkFailure::Skill { name }) => format!(
            "The session {address} {failed} because the skill \"{name}\" couldn't be loaded. \
             Check that the skill is still installed, then {again}."
        ),
        None => {
            let retry = if resume { "sending it" } else { "starting it" };
            format!(
                "The session {address} {failed} because of an unexpected problem. Try {retry} \
                 again; if it keeps failing, the agent's logs have the details."
            )
        }
    }
}

/// The sentence the Schedule place shows, and `pulse_state.json` records,
/// when a fire of the pulse `pulse` couldn't start its session: what went
/// wrong and what to do next, from the [`ForkFailure`] anywhere in `error`'s
/// context chain.
pub(crate) fn pulse_start_failure_message(pulse: &str, error: &anyhow::Error) -> String {
    let failed = format!("The pulse \"{pulse}\" couldn't start its session because");
    match error.downcast_ref::<ForkFailure>() {
        Some(ForkFailure::UnknownModel { model }) => format!(
            "{failed} the model {model} isn't available. Give the pulse another model_tier in \
             HEARTBEAT.yml."
        ),
        Some(ForkFailure::ProviderChain { model }) => format!(
            "{failed} its model ({model}) couldn't be set up, usually because a provider's API \
             key is missing. Check Settings → Model → Providers."
        ),
        Some(ForkFailure::IdentityFiles) => format!(
            "{failed} the agent's identity files couldn't be read. Check that files such as \
             SOUL.md exist and can be opened."
        ),
        Some(ForkFailure::Skill { name }) => format!(
            "{failed} the skill \"{name}\" couldn't be loaded. Check that the skill is still \
             installed."
        ),
        None => format!(
            "{failed} of an unexpected problem. It tries again at its next run; if it keeps \
             failing, the agent's logs have the details."
        ),
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context as _;

    use super::*;

    fn address() -> SessionAddress {
        SessionAddress::from("spawned-session-1a2b")
    }

    /// An error the way a fork step builds it: a cause, the step's context,
    /// then one more context from a caller further out.
    fn failed_at(step: ForkFailure) -> anyhow::Error {
        Err::<(), _>(anyhow::anyhow!("the underlying cause"))
            .context(step)
            .context("an outer caller's context")
            .unwrap_err()
    }

    fn unknown_model() -> anyhow::Error {
        failed_at(ForkFailure::UnknownModel {
            model: "lcoal/qwen3:8b".to_string(),
        })
    }

    fn provider_chain() -> anyhow::Error {
        failed_at(ForkFailure::ProviderChain {
            model: "medium".to_string(),
        })
    }

    fn skill() -> anyhow::Error {
        failed_at(ForkFailure::Skill {
            name: "researcher".to_string(),
        })
    }

    fn unclassified() -> anyhow::Error {
        anyhow::anyhow!("session address is already live; input dropped")
            .context("failed to deliver into the live run")
    }

    #[test]
    fn an_unknown_model_names_it_and_suggests_another() {
        assert_eq!(
            owner_start_failure_message(&address(), false, &unknown_model()),
            "The session spawned-session-1a2b couldn't start because the model lcoal/qwen3:8b \
             isn't available. Choose another model and start the session again."
        );
    }

    #[test]
    fn a_resume_on_an_unknown_model_suggests_a_new_session() {
        assert_eq!(
            owner_start_failure_message(&address(), true, &unknown_model()),
            "The session spawned-session-1a2b couldn't pick up your message because the model \
             lcoal/qwen3:8b isn't available. Start a new session on another model and send your \
             message there."
        );
    }

    #[test]
    fn a_provider_chain_failure_points_at_the_provider_settings() {
        assert_eq!(
            owner_start_failure_message(&address(), false, &provider_chain()),
            "The session spawned-session-1a2b couldn't start because its model (medium) \
             couldn't be set up, usually because a provider's API key is missing. Check \
             Settings → Model → Providers, then start the session again."
        );
    }

    #[test]
    fn a_resume_whose_provider_chain_fails_asks_to_send_the_message_again() {
        assert_eq!(
            owner_start_failure_message(&address(), true, &provider_chain()),
            "The session spawned-session-1a2b couldn't pick up your message because its model \
             (medium) couldn't be set up, usually because a provider's API key is missing. Check \
             Settings → Model → Providers, then send your message again."
        );
    }

    #[test]
    fn unreadable_identity_files_say_which_files() {
        assert_eq!(
            owner_start_failure_message(&address(), false, &failed_at(ForkFailure::IdentityFiles)),
            "The session spawned-session-1a2b couldn't start because the agent's identity files \
             couldn't be read. Check that files such as SOUL.md exist and can be opened, then \
             start the session again."
        );
    }

    #[test]
    fn a_resume_with_unreadable_identity_files_asks_to_send_the_message_again() {
        assert_eq!(
            owner_start_failure_message(&address(), true, &failed_at(ForkFailure::IdentityFiles)),
            "The session spawned-session-1a2b couldn't pick up your message because the agent's \
             identity files couldn't be read. Check that files such as SOUL.md exist and can be \
             opened, then send your message again."
        );
    }

    #[test]
    fn a_skill_that_wont_activate_names_the_skill() {
        assert_eq!(
            owner_start_failure_message(&address(), false, &skill()),
            "The session spawned-session-1a2b couldn't start because the skill \"researcher\" \
             couldn't be loaded. Check that the skill is still installed, then start the session \
             again."
        );
    }

    #[test]
    fn a_resume_whose_skill_wont_activate_asks_to_send_the_message_again() {
        assert_eq!(
            owner_start_failure_message(&address(), true, &skill()),
            "The session spawned-session-1a2b couldn't pick up your message because the skill \
             \"researcher\" couldn't be loaded. Check that the skill is still installed, then send \
             your message again."
        );
    }

    #[test]
    fn any_other_failure_says_to_retry_and_never_shows_the_developer_wording() {
        assert_eq!(
            owner_start_failure_message(&address(), false, &unclassified()),
            "The session spawned-session-1a2b couldn't start because of an unexpected problem. \
             Try starting it again; if it keeps failing, the agent's logs have the details."
        );
    }

    #[test]
    fn any_other_resume_failure_says_to_send_the_message_again() {
        assert_eq!(
            owner_start_failure_message(&address(), true, &unclassified()),
            "The session spawned-session-1a2b couldn't pick up your message because of an \
             unexpected problem. Try sending it again; if it keeps failing, the agent's logs \
             have the details."
        );
    }

    #[test]
    fn a_pulse_whose_model_is_unknown_names_it_and_points_at_heartbeat_yml() {
        assert_eq!(
            pulse_start_failure_message("watch", &unknown_model()),
            "The pulse \"watch\" couldn't start its session because the model lcoal/qwen3:8b \
             isn't available. Give the pulse another model_tier in HEARTBEAT.yml."
        );
    }

    #[test]
    fn a_pulse_whose_provider_chain_fails_points_at_the_provider_settings() {
        assert_eq!(
            pulse_start_failure_message("watch", &provider_chain()),
            "The pulse \"watch\" couldn't start its session because its model (medium) couldn't \
             be set up, usually because a provider's API key is missing. Check Settings → Model \
             → Providers."
        );
    }

    #[test]
    fn a_pulse_with_unreadable_identity_files_says_which_files() {
        assert_eq!(
            pulse_start_failure_message("watch", &failed_at(ForkFailure::IdentityFiles)),
            "The pulse \"watch\" couldn't start its session because the agent's identity files \
             couldn't be read. Check that files such as SOUL.md exist and can be opened."
        );
    }

    #[test]
    fn a_pulse_whose_skill_wont_activate_names_the_skill() {
        assert_eq!(
            pulse_start_failure_message("watch", &skill()),
            "The pulse \"watch\" couldn't start its session because the skill \"researcher\" \
             couldn't be loaded. Check that the skill is still installed."
        );
    }

    #[test]
    fn any_other_pulse_failure_never_shows_the_developer_wording() {
        assert_eq!(
            pulse_start_failure_message("watch", &unclassified()),
            "The pulse \"watch\" couldn't start its session because of an unexpected problem. \
             It tries again at its next run; if it keeps failing, the agent's logs have the \
             details."
        );
    }

    #[test]
    fn the_step_heads_the_cause_chain_in_the_details() {
        let error = Err::<(), _>(anyhow::anyhow!("anthropic requires an API key"))
            .context(ForkFailure::ProviderChain {
                model: "medium".to_string(),
            })
            .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "failed to build provider chain for model medium: anthropic requires an API key"
        );
    }
}
