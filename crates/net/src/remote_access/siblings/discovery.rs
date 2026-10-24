//! What sibling discovery needs from remote access while the secure tunnel is
//! up: where the user's agent directory is, the credential that shows private
//! agents in it, and the joined siblings with the keys to call them.

use std::collections::BTreeMap;
use std::sync::Arc;

/// Builds a sibling's origin (`https://{slug}.{user}.{base}`) from its slug.
pub(crate) type OriginFor = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Everything discovery uses. Nothing in it comes from a relay frame except
/// the directory token, and that goes only to the directory URL derived here.
#[derive(Clone)]
pub struct SecureDiscovery {
    /// This instance's slug, which discovery leaves out.
    pub own_slug: String,
    /// `https://{base}/a2a/{user}/agents`, derived from the stored identity.
    pub directory_url: String,
    /// Lets the directory list the user's private agents.
    pub directory_token: Option<String>,
    /// The joined siblings and the key to present to each. Only these are registered.
    pub siblings: BTreeMap<String, String>,
    /// A sibling's origin from its slug.
    pub origin_for: OriginFor,
}

impl std::fmt::Debug for SecureDiscovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureDiscovery")
            .field("own_slug", &self.own_slug)
            .field("directory_url", &self.directory_url)
            .field("siblings", &self.siblings.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}
