use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) type OriginFor = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Clone)]
pub struct SecureDiscovery {
    pub own_slug: String,
    pub directory_url: String,
    pub directory_token: Option<String>,
    pub siblings: BTreeMap<String, String>,
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
