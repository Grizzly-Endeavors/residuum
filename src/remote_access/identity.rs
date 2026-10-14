//! Identity is local: the user and slug an install enrolled as, and the check
//! that what the relay announces agrees with it.

use super::store::LocalIdentity;
use super::types::Hostnames;

/// Why a `Connected` frame was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum IdentityError {
    /// The relay says this tunnel belongs to someone else than the stored identity.
    #[error(
        "the relay says this instance is {announced_instance} of {announced_user}, but it was set up as {stored_instance} of {stored_user}"
    )]
    Mismatch {
        announced_user: String,
        announced_instance: String,
        stored_user: String,
        stored_instance: String,
    },
    /// The relay announced host names other than the ones derived locally.
    #[error("the relay announced host names that differ from the ones derived from {base_domain}")]
    HostsDiffer { base_domain: String },
    /// The user or slug isn't a valid name.
    #[error("the relay announced a user or instance name that isn't valid")]
    InvalidName,
}

/// Check what the relay announced against the stored identity (when there is
/// one) and the host names derived from `base_domain`.
///
/// # Errors
/// Returns why the announcement is refused.
pub(crate) fn verify_announcement(
    stored: Option<&LocalIdentity>,
    base_domain: &str,
    announced_user: &str,
    announced_instance: &str,
    announced_hosts: &Hostnames,
) -> Result<(), IdentityError> {
    if !is_valid_user(announced_user) || !is_valid_slug(announced_instance) {
        return Err(IdentityError::InvalidName);
    }
    if let Some(stored) = stored
        && (stored.user != announced_user || stored.slug != announced_instance)
    {
        return Err(IdentityError::Mismatch {
            announced_user: announced_user.to_string(),
            announced_instance: announced_instance.to_string(),
            stored_user: stored.user.clone(),
            stored_instance: stored.slug.clone(),
        });
    }
    let derived = Hostnames::derive(announced_user, announced_instance, base_domain);
    if derived != normalized(announced_hosts) {
        return Err(IdentityError::HostsDiffer {
            base_domain: base_domain.to_string(),
        });
    }
    Ok(())
}

fn normalized(hosts: &Hostnames) -> Hostnames {
    Hostnames {
        ui: super::types::normalize_host(&hosts.ui),
        workbench: super::types::normalize_host(&hosts.workbench),
        instance: super::types::normalize_host(&hosts.instance),
    }
}

/// A username as the relay and the pin service accept it: 3-32 characters of
/// `a-z`, `0-9` and inner `-`.
pub(crate) fn is_valid_user(user: &str) -> bool {
    (3..=32).contains(&user.len()) && is_dns_label(user)
}

/// An instance slug: 1-24 characters of `a-z`, `0-9` and inner `-`.
pub(crate) fn is_valid_slug(slug: &str) -> bool {
    (1..=24).contains(&slug.len()) && is_dns_label(slug)
}

fn is_dns_label(text: &str) -> bool {
    !text.starts_with('-')
        && !text.ends_with('-')
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "agent-residuum.com";

    fn stored() -> LocalIdentity {
        LocalIdentity {
            user: "bear".into(),
            slug: "laptop".into(),
        }
    }

    fn hosts(user: &str, slug: &str) -> Hostnames {
        Hostnames::derive(user, slug, BASE)
    }

    #[test]
    fn a_matching_announcement_passes() {
        assert!(
            verify_announcement(
                Some(&stored()),
                BASE,
                "bear",
                "laptop",
                &hosts("bear", "laptop")
            )
            .is_ok()
        );
        assert!(
            verify_announcement(None, BASE, "bear", "laptop", &hosts("bear", "laptop")).is_ok(),
            "before enrollment the announcement is taken once, and enrollment is what stores it"
        );
    }

    #[test]
    fn a_different_user_or_instance_is_refused() {
        for (user, slug) in [("mallory", "laptop"), ("bear", "desktop")] {
            let err = verify_announcement(Some(&stored()), BASE, user, slug, &hosts(user, slug))
                .unwrap_err();
            assert!(
                matches!(err, IdentityError::Mismatch { .. }),
                "{user}/{slug}"
            );
        }
    }

    #[test]
    fn announced_hosts_must_equal_the_derived_ones() {
        let mut lying = hosts("bear", "laptop");
        lying.ui = "bear.evil.example".into();
        assert!(matches!(
            verify_announcement(Some(&stored()), BASE, "bear", "laptop", &lying),
            Err(IdentityError::HostsDiffer { .. })
        ));
    }

    #[test]
    fn names_are_validated() {
        assert!(is_valid_user("bear"));
        assert!(!is_valid_user("be"));
        assert!(!is_valid_user("Bear"));
        assert!(!is_valid_user("bear."));
        assert!(is_valid_slug("home-1"));
        assert!(!is_valid_slug("a.b"));
        assert!(!is_valid_slug(&"x".repeat(25)));
        assert_eq!(
            verify_announcement(None, BASE, "b.ear", "x", &hosts("b.ear", "x")),
            Err(IdentityError::InvalidName)
        );
    }
}
