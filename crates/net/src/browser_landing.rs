//! What a person's browser gets when it lands on a Residuum address that isn't
//! a page of the web app: a file link that expired, an artifact that was
//! renamed, a webhook address pasted into the address bar.
//!
//! Wherever the web app can show the outcome, a browser navigation is
//! redirected into it ([`app_redirect`]) with a notice code the app turns into
//! a plain-language message (`web/src/lib/landing-notices.ts`). Where there is
//! no app to hand off to (a host that doesn't know the app's address), the
//! navigation gets [`fallback_page`], one page styled from the app's own
//! design tokens. Requests that aren't navigations (a script's `fetch`, an
//! image or script load) keep machine-shaped answers.

use std::sync::LazyLock;

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};

pub(crate) use crate::pairing::remote::is_navigation;

/// Whether the request loads a whole tab, which can be handed to the app,
/// rather than a frame inside another page, which can't: the app refuses to
/// run inside a frame, and a frame showing it would be the app in miniature.
///
/// A navigation without fetch metadata (an older browser) counts as a tab.
#[must_use]
pub fn is_top_level_navigation(method: &Method, headers: &HeaderMap) -> bool {
    is_navigation(method, headers)
        && headers
            .get("sec-fetch-dest")
            .and_then(|v| v.to_str().ok())
            .is_none_or(|dest| dest.eq_ignore_ascii_case("document"))
}

/// A notice the web app shows after a redirect, named by its code in the
/// `notice` query parameter. The app holds the words, so a link can't put
/// text of its own in front of the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppNotice {
    /// No artifact by that name (named in the `artifact` parameter when the
    /// name is well formed).
    ArtifactMissing,
    /// The artifact exists, but not the file the address asked for.
    ArtifactFileMissing,
    /// A token file link outlived its registration.
    FileLinkExpired,
    /// The link needs the agent running, and it isn't.
    AgentStopped,
    /// An inbox item's attachment is gone.
    InboxAttachmentMissing,
    /// A webhook address opened in a browser.
    WebhookAddress,
}

impl AppNotice {
    /// The code the app reads from the `notice` query parameter.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::ArtifactMissing => "artifact-missing",
            Self::ArtifactFileMissing => "artifact-file-missing",
            Self::FileLinkExpired => "file-link-expired",
            Self::AgentStopped => "agent-stopped",
            Self::InboxAttachmentMissing => "inbox-attachment-missing",
            Self::WebhookAddress => "webhook-address",
        }
    }
}

/// An address in the web app: a path and its query parameters, encoded.
#[must_use]
pub fn app_path(path: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() {
        return path.to_string();
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in params {
        query.append_pair(name, value);
    }
    format!("{path}?{}", query.finish())
}

/// What to show instead of a redirect into the app when the address the
/// redirect would need isn't known (see [`AppRedirect`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fallback {
    pub title: &'static str,
    pub message: &'static str,
}

/// Marks a response as a redirect into the web app by an app-relative path,
/// so a host other than the app's (the artifacts listener, which forwards
/// `/api` in-process) can point it at the app's origin, or show the fallback
/// when it doesn't know that origin.
#[derive(Debug, Clone, Copy)]
pub struct AppRedirect {
    pub(crate) fallback: Fallback,
}

/// Redirect a navigation to `target`, an address in the web app (from
/// [`app_path`]) on this same host.
pub fn app_redirect(target: &str, fallback: Fallback) -> Response {
    let mut response = Redirect::to(target).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.extensions_mut().insert(AppRedirect { fallback });
    response
}

/// Redirect a navigation to `target`, an absolute address in the web app on
/// another host.
pub(crate) fn redirect_to_app_origin(origin: &str, target: &str) -> Response {
    let mut response = Redirect::to(&format!("{origin}{target}")).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// A link on the fallback page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageLink {
    pub(crate) href: String,
    pub(crate) label: &'static str,
}

/// What the fallback page says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) detail: Option<String>,
    pub(crate) link: Option<PageLink>,
}

impl Landing {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            detail: None,
            link: None,
        }
    }

    /// Add a link to the app at `origin` (when known), labelled `label`.
    #[must_use]
    pub(crate) fn with_app_link(mut self, origin: Option<&str>, label: &'static str) -> Self {
        self.link = origin.map(|href| PageLink {
            href: href.to_string(),
            label,
        });
        self
    }

    /// Add a second paragraph under the message.
    #[must_use]
    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl From<Fallback> for Landing {
    fn from(fallback: Fallback) -> Self {
        Self::new(fallback.title, fallback.message)
    }
}

/// The fallback page for a browser navigation, with `status`.
pub fn fallback_page(status: StatusCode, landing: &Landing) -> Response {
    let link = landing.link.as_ref().map_or_else(String::new, |link| {
        format!(
            r#"<p class="actions"><a href="{}">{}</a></p>"#,
            escape(&link.href),
            escape(link.label)
        )
    });
    let detail = landing
        .detail
        .as_ref()
        .map_or_else(String::new, |detail| format!("<p>{}</p>", escape(detail)));
    let body = format!(
        "<h1>{}</h1><p>{}</p>{detail}{link}",
        escape(&landing.title),
        escape(&landing.message)
    );
    html_response(status, &landing.title, &body, None)
}

/// A machine-shaped answer for a request that isn't a navigation: the same
/// words as plain text, so a script or a log shows why.
pub(crate) fn text_answer(status: StatusCode, message: &str) -> Response {
    let mut response = (status, format!("{message}\n")).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// The fallback page for a navigation, plain text for anything else.
pub(crate) fn page_or_text(
    method: &Method,
    headers: &HeaderMap,
    status: StatusCode,
    landing: &Landing,
) -> Response {
    if is_navigation(method, headers) {
        fallback_page(status, landing)
    } else {
        text_answer(status, &landing.message)
    }
}

/// A page in the fallback's frame: `title` in the tab, `body` (already
/// escaped HTML) in its column, and `script` run at the end when given.
pub(crate) fn html_response(
    status: StatusCode,
    title: &str,
    body: &str,
    script: Option<&str>,
) -> Response {
    let script_policy = if script.is_some() {
        "'unsafe-inline'"
    } else {
        "'none'"
    };
    let script = script.map_or_else(String::new, |s| format!("<script>{s}</script>"));
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="dark"><meta name="robots" content="noindex"><title>{} · Residuum</title><style>{}</style></head><body><header><span class="wordmark">{MARK_SVG}Residuum</span></header><main>{body}</main>{script}</body></html>"#,
        escape(title),
        *PAGE_CSS,
    );
    let mut response = (status, html).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(policy) = HeaderValue::from_str(&format!(
        "default-src 'none'; style-src 'unsafe-inline'; script-src {script_policy}; connect-src 'self'; base-uri 'none'; form-action 'none'"
    )) {
        headers.insert(header::CONTENT_SECURITY_POLICY, policy);
    }
    response
}

/// Escape text for HTML content and attribute values.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The app's `mark` icon (`web/src/lib/icons/icons.ts`), drawn as `Icon` draws it.
const MARK_SVG: &str = r#"<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 2.5 21.5 12 12 21.5 2.5 12z"/><path d="M8.6 12 12 8.6"/></svg>"#;

/// The app's design tokens. The page reads its values from here, so it
/// always matches the app.
const TOKENS_CSS: &str = include_str!("../../../web/src/styles/tokens.css");

/// The tokens the page's rules use.
const PAGE_TOKENS: [&str; 26] = [
    "--color-stone-0",
    "--color-line-soft",
    "--color-text",
    "--color-text-2",
    "--color-vein",
    "--color-vein-bright",
    "--color-vein-dim",
    "--color-vein-hover",
    "--color-on-accent",
    "--font-ui",
    "--font-mark",
    "--font-size-sm",
    "--font-size-ui",
    "--font-size-title",
    "--font-weight-medium",
    "--font-weight-semibold",
    "--line-height-tight",
    "--line-height-ui",
    "--corner-sm",
    "--space-10",
    "--space-12",
    "--space-16",
    "--space-20",
    "--space-32",
    "--layout-place-header-height",
    "--layout-touch-target",
];

/// The page's rules, after the token declarations they read. Laid out like
/// the pairing page, the app's other page without the shell: the wordmark in
/// a header bar, then a 480px column with the title, the words, and the one
/// action as a primary button.
const PAGE_RULES: &str = "\
*{box-sizing:border-box}\
html{color-scheme:dark}\
body{margin:0;min-height:100vh;background:var(--color-stone-0);color:var(--color-text);font-family:var(--font-ui);font-size:var(--font-size-ui);line-height:var(--line-height-ui);-webkit-font-smoothing:antialiased}\
header{display:flex;align-items:center;height:var(--layout-place-header-height);padding:0 var(--space-20);border-bottom:1px solid var(--color-line-soft)}\
.wordmark{display:inline-flex;align-items:center;gap:var(--space-10);font-family:var(--font-mark);font-size:var(--font-size-sm);font-weight:var(--font-weight-medium);letter-spacing:.2em;text-transform:uppercase}\
.wordmark svg{color:var(--color-vein)}\
main{display:flex;flex-direction:column;gap:var(--space-16);max-width:calc(480px + 2 * var(--space-16));margin:0 auto;padding:var(--space-32) var(--space-16)}\
h1{margin:0;font-size:var(--font-size-title);font-weight:var(--font-weight-semibold);line-height:var(--line-height-tight)}\
p{margin:0;color:var(--color-text-2)}\
[hidden]{display:none!important}\
.actions a{display:inline-flex;align-items:center;height:32px;padding:0 var(--space-12);border-radius:var(--corner-sm);background:var(--color-vein-dim);color:var(--color-on-accent);font-size:var(--font-size-sm);font-weight:var(--font-weight-medium);text-decoration:none}\
.actions a:hover{background:var(--color-vein-hover)}\
a:focus-visible{outline:2px solid var(--color-vein-bright);outline-offset:2px}\
@media (max-width:760px){.actions a{height:var(--layout-touch-target)}}";

static PAGE_CSS: LazyLock<String> = LazyLock::new(|| {
    let mut css = String::from(":root{");
    for name in PAGE_TOKENS {
        match token_value(TOKENS_CSS, name) {
            Some(value) => {
                css.push_str(name);
                css.push(':');
                css.push_str(value);
                css.push(';');
            }
            None => {
                tracing::error!(
                    token = name,
                    "a design token the fallback page uses is missing from tokens.css"
                );
            }
        }
    }
    css.push('}');
    css.push_str(PAGE_RULES);
    css
});

/// The value `tokens` declares for the custom property `name`.
fn token_value<'a>(tokens: &'a str, name: &str) -> Option<&'a str> {
    tokens.lines().find_map(|line| {
        let rest = line
            .trim()
            .strip_prefix(name)?
            .trim_start()
            .strip_prefix(':')?;
        Some(rest.trim().strip_suffix(';')?.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn every_token_the_page_uses_is_in_the_apps_tokens() {
        for name in PAGE_TOKENS {
            assert!(
                token_value(TOKENS_CSS, name).is_some(),
                "{name} is missing from web/src/styles/tokens.css"
            );
        }
        assert_eq!(
            token_value(TOKENS_CSS, "--color-stone-0"),
            Some("#0e0e10"),
            "reads a declaration's value without its semicolon"
        );
        assert_eq!(
            token_value(TOKENS_CSS, "--color-stone"),
            None,
            "a prefix of a token's name is not that token"
        );
    }

    #[test]
    fn every_rule_reads_only_declared_tokens() {
        let mut rest = PAGE_RULES;
        while let Some(start) = rest.find("var(") {
            let after = rest.get(start + 4..).unwrap_or_default();
            let end = after.find(')').unwrap();
            let name = after.get(..end).unwrap();
            assert!(PAGE_TOKENS.contains(&name), "{name} isn't in PAGE_TOKENS");
            rest = after.get(end..).unwrap_or_default();
        }
    }

    #[test]
    fn a_tab_is_a_top_level_navigation_and_a_frame_is_not() {
        let tab = headers(&[
            ("sec-fetch-dest", "document"),
            ("sec-fetch-mode", "navigate"),
        ]);
        assert!(is_top_level_navigation(&Method::GET, &tab), "a tab");
        let old_browser = headers(&[("accept", "text/html,*/*")]);
        assert!(
            is_top_level_navigation(&Method::GET, &old_browser),
            "no fetch metadata"
        );
        let frame = headers(&[("sec-fetch-dest", "iframe"), ("sec-fetch-mode", "navigate")]);
        assert!(is_navigation(&Method::GET, &frame), "a frame navigates");
        assert!(
            !is_top_level_navigation(&Method::GET, &frame),
            "but not a whole tab"
        );
        let fetch = headers(&[("sec-fetch-dest", "empty"), ("accept", "*/*")]);
        assert!(!is_top_level_navigation(&Method::GET, &fetch), "a fetch");
    }

    #[test]
    fn app_paths_encode_their_parameters() {
        assert_eq!(app_path("/home", &[]), "/home");
        assert_eq!(
            app_path(
                "/agent/scout/files",
                &[("panel", "file:notes/a b&c.md"), ("notice", "x")]
            ),
            "/agent/scout/files?panel=file%3Anotes%2Fa+b%26c.md&notice=x"
        );
    }

    #[tokio::test]
    async fn the_fallback_page_escapes_what_it_shows_and_carries_the_tokens() {
        let landing = Landing::new("No <artifact>", "There's no \"x\" & more.")
            .with_app_link(Some("https://bear.example"), "Open Residuum");
        let response = fallback_page(StatusCode::NOT_FOUND, &landing);
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "keeps the status");
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8",
            "an HTML page"
        );
        let policy = response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(policy.contains("script-src 'none'"), "{policy}");
        let page = body_text(response).await;
        assert!(page.contains("<h1>No &lt;artifact&gt;</h1>"), "{page}");
        assert!(
            page.contains("There&#39;s no &quot;x&quot; &amp; more."),
            "{page}"
        );
        assert!(
            page.contains(r#"<a href="https://bear.example">Open Residuum</a>"#),
            "{page}"
        );
        assert!(page.contains("--color-stone-0:#0e0e10"), "{page}");
    }

    #[tokio::test]
    async fn a_fetch_gets_the_words_as_text() {
        let landing = Landing::new("Gone", "That file is gone.");
        let fetch = headers(&[("accept", "*/*")]);
        let response = page_or_text(&Method::GET, &fetch, StatusCode::NOT_FOUND, &landing);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8",
            "plain text"
        );
        assert_eq!(
            body_text(response).await,
            "That file is gone.\n",
            "the words"
        );
    }

    #[test]
    fn an_app_redirect_is_marked_for_other_hosts() {
        let fallback = Fallback {
            title: "t",
            message: "m",
        };
        let response = app_redirect("/home?notice=webhook-address", fallback);
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "a redirect");
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/home?notice=webhook-address",
            "to the app path"
        );
        assert_eq!(
            response.extensions().get::<AppRedirect>().unwrap().fallback,
            fallback,
            "marked"
        );
    }
}
