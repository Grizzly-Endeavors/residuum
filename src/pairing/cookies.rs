//! The device credential cookie: its name, its `Set-Cookie` line, and finding
//! it in a request's `Cookie` header.

use axum::http::{HeaderMap, header};

use super::state::DEVICE_LIFETIME_DAYS;

/// Prefix of the cookie's name. `__Host-` makes browsers accept it only
/// with `Secure`, `Path=/` and no `Domain`, so it stays on the one host that
/// set it. The instance slug follows, so several instances can coexist on the
/// shared UI host.
const NAME_PREFIX: &str = "__Host-residuum_device_";

/// The slug used when Residuum Cloud has not announced one.
pub(super) const FALLBACK_SLUG: &str = "default";

/// The longest slug the relay allows.
const MAX_SLUG_LEN: usize = 24;

/// Whether `slug` is one the relay could have announced: 1 to 24 lowercase
/// letters, digits or hyphens, not starting or ending with a hyphen. Anything
/// else could not be part of a cookie name safely.
pub(super) fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

/// The cookie's name for the instance with `slug`.
pub(super) fn cookie_name(slug: Option<&str>) -> String {
    format!("{NAME_PREFIX}{}", slug.unwrap_or(FALLBACK_SLUG))
}

/// The `Set-Cookie` value that stores `secret` for 400 days.
pub(super) fn set_cookie_value(name: &str, secret: &str) -> String {
    let max_age = DEVICE_LIFETIME_DAYS * 24 * 60 * 60;
    format!("{name}={secret}; Max-Age={max_age}; Path=/; Secure; HttpOnly; SameSite=Lax")
}

/// Every cookie a request carries, as one `Cookie` header value. Over HTTP/2
/// a browser may send each cookie as a field of its own, so reading only the
/// first field would miss the rest.
pub(crate) fn cookie_header(headers: &HeaderMap) -> Option<String> {
    let fields: Vec<&str> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    (!fields.is_empty()).then(|| fields.join("; "))
}

/// The value of cookie `name` in a `Cookie` header, if it is there.
pub(super) fn find_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name && !value.is_empty()).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_carries_the_slug_or_a_fallback() {
        assert_eq!(cookie_name(Some("laptop")), "__Host-residuum_device_laptop");
        assert_eq!(cookie_name(None), "__Host-residuum_device_default");
    }

    #[test]
    fn the_cookie_is_host_only_secure_http_only_lax_and_lasts_400_days() {
        let line = set_cookie_value("__Host-residuum_device_laptop", "abc");
        assert!(line.starts_with("__Host-residuum_device_laptop=abc;"));
        for part in [
            "Max-Age=34560000",
            "Path=/",
            "Secure",
            "HttpOnly",
            "SameSite=Lax",
        ] {
            assert!(line.contains(part), "missing {part} in {line}");
        }
        assert!(
            !line.contains("Domain"),
            "a __Host- cookie must not set Domain"
        );
    }

    #[test]
    fn finds_the_cookie_among_others() {
        let header = "a=1; __Host-residuum_device_laptop=secret; b=2";
        assert_eq!(
            find_cookie(header, "__Host-residuum_device_laptop"),
            Some("secret")
        );
        assert_eq!(find_cookie(header, "__Host-residuum_device_phone"), None);
        assert_eq!(find_cookie("x=", "x"), None);
    }

    #[test]
    fn cookies_split_across_fields_are_joined() {
        let mut headers = HeaderMap::new();
        assert_eq!(cookie_header(&headers), None);
        headers.append(header::COOKIE, "a=1".parse().unwrap());
        headers.append(header::COOKIE, "b=2; c=3".parse().unwrap());
        assert_eq!(cookie_header(&headers).as_deref(), Some("a=1; b=2; c=3"));
    }

    #[test]
    fn slugs_follow_the_relays_rules() {
        assert!(is_valid_slug("laptop"));
        assert!(is_valid_slug("home-1"));
        for bad in ["", "-a", "a-", "UPPER", "a b", "a;b", &"x".repeat(25)] {
            assert!(!is_valid_slug(bad), "{bad:?}");
        }
    }
}
