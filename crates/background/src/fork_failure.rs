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
/// couldn't start: what went wrong and what to do next, from the
/// [`ForkFailure`] anywhere in `error`'s context chain.
pub(crate) fn owner_start_failure_message(
    address: &SessionAddress,
    error: &anyhow::Error,
) -> String {
    match error.downcast_ref::<ForkFailure>() {
        Some(ForkFailure::UnknownModel { model }) => format!(
            "The session {address} couldn't start because the model {model} isn't available. \
             Choose another model and start the session again."
        ),
        Some(ForkFailure::ProviderChain { model }) => format!(
            "The session {address} couldn't start because its model ({model}) couldn't be set \
             up, usually because a provider's API key is missing or wrong. Check Settings → \
             Model → Providers, then start the session again."
        ),
        Some(ForkFailure::IdentityFiles) => format!(
            "The session {address} couldn't start because the agent's identity files couldn't \
             be read. Check that files such as SOUL.md exist and can be opened, then start the \
             session again."
        ),
        Some(ForkFailure::Skill { name }) => format!(
            "The session {address} couldn't start because the skill \"{name}\" couldn't be \
             loaded. Check that the skill is still installed, then start the session again."
        ),
        None => format!(
            "The session {address} couldn't start because of an unexpected problem. Try \
             starting it again; if it keeps failing, the agent's logs have the details."
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

    #[test]
    fn an_unknown_model_names_it_and_suggests_another() {
        let message = owner_start_failure_message(
            &address(),
            &failed_at(ForkFailure::UnknownModel {
                model: "lcoal/qwen3:8b".to_string(),
            }),
        );
        assert_eq!(
            message,
            "The session spawned-session-1a2b couldn't start because the model lcoal/qwen3:8b \
             isn't available. Choose another model and start the session again."
        );
    }

    #[test]
    fn a_provider_chain_failure_points_at_the_provider_settings() {
        let message = owner_start_failure_message(
            &address(),
            &failed_at(ForkFailure::ProviderChain {
                model: "medium".to_string(),
            }),
        );
        assert_eq!(
            message,
            "The session spawned-session-1a2b couldn't start because its model (medium) \
             couldn't be set up, usually because a provider's API key is missing or wrong. \
             Check Settings → Model → Providers, then start the session again."
        );
    }

    #[test]
    fn unreadable_identity_files_say_which_files() {
        let message =
            owner_start_failure_message(&address(), &failed_at(ForkFailure::IdentityFiles));
        assert_eq!(
            message,
            "The session spawned-session-1a2b couldn't start because the agent's identity files \
             couldn't be read. Check that files such as SOUL.md exist and can be opened, then \
             start the session again."
        );
    }

    #[test]
    fn a_skill_that_wont_activate_names_the_skill() {
        let message = owner_start_failure_message(
            &address(),
            &failed_at(ForkFailure::Skill {
                name: "researcher".to_string(),
            }),
        );
        assert_eq!(
            message,
            "The session spawned-session-1a2b couldn't start because the skill \"researcher\" \
             couldn't be loaded. Check that the skill is still installed, then start the session \
             again."
        );
    }

    #[test]
    fn any_other_failure_says_to_retry_and_never_shows_the_developer_wording() {
        let error = anyhow::anyhow!("session address is already live; input dropped")
            .context("failed to deliver into the live run");
        let message = owner_start_failure_message(&address(), &error);
        assert_eq!(
            message,
            "The session spawned-session-1a2b couldn't start because of an unexpected problem. \
             Try starting it again; if it keeps failing, the agent's logs have the details."
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
