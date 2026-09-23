//! The artifact identity the workbench bridge attaches to every request it
//! relays for an artifact.
//!
//! The bridge (web UI code) sets `X-Residuum-Artifact: <name>` on every
//! request it makes on an artifact's behalf, replacing any value the artifact
//! supplied. The gateway's cross-site guard means only the web UI's own origin
//! can make state-changing requests, so the header names the artifact whose
//! frame made the request. Endpoints that attribute work to an artifact read
//! it through [`artifact_identity`].

use axum::http::HeaderMap;

use crate::workbench::is_valid_artifact_name;

/// The header carrying the requesting artifact's name.
pub(crate) const ARTIFACT_HEADER: &str = "x-residuum-artifact";

/// The artifact named by the request's identity header: `Ok(None)` when the
/// header is absent (the web UI itself, or another API client).
///
/// # Errors
/// A plain-language explanation when the header is present but isn't a
/// valid artifact name, which only a request not made through the bridge
/// could send.
pub(crate) fn artifact_identity(headers: &HeaderMap) -> Result<Option<String>, String> {
    let Some(value) = headers.get(ARTIFACT_HEADER) else {
        return Ok(None);
    };
    let name = value
        .to_str()
        .ok()
        .filter(|name| is_valid_artifact_name(name))
        .ok_or_else(|| {
            format!("the {ARTIFACT_HEADER} header must name an artifact, like \"wiki-graph\"")
        })?;
    Ok(Some(name.to_string()))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(ARTIFACT_HEADER, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn absent_header_is_no_identity() {
        assert_eq!(artifact_identity(&headers(None)), Ok(None));
    }

    #[test]
    fn valid_name_is_the_identity_whatever_the_header_casing() {
        let mut map = HeaderMap::new();
        map.insert(
            "X-Residuum-Artifact",
            HeaderValue::from_static("wiki-graph"),
        );
        assert_eq!(artifact_identity(&map), Ok(Some("wiki-graph".to_string())));
    }

    #[test]
    fn invalid_names_are_refused() {
        for bad in ["", "Wiki", "../etc", "a/b", "wiki.html", "-wiki"] {
            assert!(
                artifact_identity(&headers(Some(bad))).is_err(),
                "{bad:?} should be refused"
            );
        }
    }
}
