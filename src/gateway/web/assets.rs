//! The embedded web app: static file serving with cache headers and compression.
//!
//! `web/dist/` is embedded into the binary via `rust-embed`. In debug builds,
//! files are served from disk (hot-reload); in release builds, they are
//! compiled into the binary.
//!
//! - Files under `assets/` are named by content hash, so a browser keeps them
//!   for a year without asking again.
//! - HTML documents (`index.html`, also as the answer for a client route) are
//!   fetched whole on every load: `no-cache` and no validator, so they never get
//!   a 304.
//! - Every other file (the service worker, the manifest, icons) keeps its name
//!   across releases, so a browser revalidates it on every use against a
//!   content-hash `ETag` and gets a bodyless 304 when nothing changed.
//! - Scripts, styles, JSON, SVG and the manifest are compressed when the client
//!   accepts it. HTML and images are not.

use axum::Router;
use axum::body::Body;
use axum::http::header;
use axum::http::{
    Extensions, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, Version,
};
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use tower_http::compression::{CompressionLayer, CompressionLevel};
use tracing::warn;

use super::WebAssets;

/// Directory of build output whose file names carry a content hash.
const HASHED_ASSET_DIR: &str = "assets/";

/// A hashed file never changes under its name, so it is kept for a year.
const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// `no-cache` lets a browser keep the file but makes it check before each use.
/// With an `ETag` that check is a 304; without one it is a full fetch.
const REVALIDATE_CACHE_CONTROL: &str = "no-cache";

/// Brotli quality and gzip level for compressing on the fly. The encoder's
/// default brotli quality (11) takes about 0.7 s per request on the 566 kB
/// main bundle; 5 takes about 10 ms and is within 12% of its size.
const COMPRESSION_QUALITY: i32 = 5;

/// The embedded web app as a router, for use as the hub's fallback: every
/// path the API and webhook routes don't claim lands here.
pub(crate) fn static_assets() -> Router {
    Router::new().fallback(static_handler).layer(
        CompressionLayer::new()
            .quality(CompressionLevel::Precise(COMPRESSION_QUALITY))
            .compress_when(should_compress),
    )
}

/// Serves the file at the requested URI path, falling back to `index.html`
/// for the web UI's client-side routes (paths without file extensions).
/// Unknown API and WebSocket paths get a 404 rather than the app shell, so a
/// client calling a missing endpoint sees the failure instead of HTML.
async fn static_handler(method: Method, uri: Uri, request_headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try the exact path first
    if let Some(resp) = serve_embedded(path, &method, &request_headers) {
        return resp;
    }

    if is_client_route(path)
        && let Some(resp) = serve_embedded("index.html", &method, &request_headers)
    {
        return resp;
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("not found"))
        .unwrap_or_default()
}

/// Whether `path` (without its leading slash) is a web UI route that the
/// client-side router handles, rather than a missing asset or server endpoint.
fn is_client_route(path: &str) -> bool {
    let first_segment = path.split('/').next().unwrap_or_default();
    !path.contains('.') && first_segment != "api" && first_segment != "ws"
}

/// Serve an embedded file by path, returning `None` if it doesn't exist.
///
/// Carries the cache headers for the file's class. A file that is neither
/// hashed nor HTML has an `ETag`, and a matching `If-None-Match` gets a
/// bodyless 304.
fn serve_embedded(path: &str, method: &Method, request_headers: &HeaderMap) -> Option<Response> {
    let asset = WebAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    let mut headers = HeaderMap::new();
    if is_compressible(mime.essence_str()) {
        // Sent on every answer, compressed or not, so a cache keeps the
        // encodings apart. A 304 repeats it (RFC 9110 §15.4.5).
        headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }

    let hashed = path.starts_with(HASHED_ASSET_DIR);
    let cache_control = if hashed {
        IMMUTABLE_CACHE_CONTROL
    } else {
        REVALIDATE_CACHE_CONTROL
    };
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );

    // An HTML document has no `ETag`, so a browser's revalidation is a full
    // fetch, never a 304. Residuum Cloud's relay rewrites top-level HTML on
    // the way out, adding an instance switcher that shows which instances are
    // connected at that moment, and the browser caches the rewritten body. A
    // 304 would keep showing that switcher's old state.
    if !hashed && !is_html(mime.essence_str()) {
        let etag = format!(
            "\"{}\"",
            URL_SAFE_NO_PAD.encode(asset.metadata.sha256_hash())
        );
        set_header(&mut headers, header::ETAG, &etag);

        let conditional = matches!(*method, Method::GET | Method::HEAD);
        if conditional && if_none_match_names(request_headers, &etag) {
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::NOT_MODIFIED;
            *resp.headers_mut() = headers;
            return Some(resp);
        }
    }

    set_header(&mut headers, header::CONTENT_TYPE, mime.as_ref());
    let mut resp = Response::new(Body::from(asset.data));
    *resp.headers_mut() = headers;
    Some(resp)
}

/// Whether the request's `If-None-Match` lists `etag` or `*`.
///
/// Compares weakly, as RFC 9110 §13.1.2 prescribes for this header, so a
/// `W/` prefix on the client's copy doesn't hide a match.
fn if_none_match_names(request_headers: &HeaderMap, etag: &str) -> bool {
    request_headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
        })
}

/// Set a header from a string that is valid by construction. A value that
/// isn't is logged and the header left off, so the file is still served.
fn set_header(headers: &mut HeaderMap, name: HeaderName, value: &str) {
    match HeaderValue::from_str(value) {
        Ok(value) => {
            headers.insert(name, value);
        }
        Err(error) => {
            warn!(header = %name, error = %error, "dropping an invalid static file header");
        }
    }
}

/// Whether this media type (no parameters) is an HTML document.
fn is_html(essence: &str) -> bool {
    essence == "text/html"
}

/// Whether a response with this media type (no parameters) is worth
/// compressing.
///
/// HTML is left out on purpose. Residuum Cloud's relay inserts its instance
/// switcher before the `</body>` of a top-level page, and it finds that tag by
/// searching the body as text. In a compressed body it finds nothing and sends
/// the page without a switcher. Images are already compressed.
fn is_compressible(essence: &str) -> bool {
    matches!(
        essence,
        "text/javascript"
            | "application/javascript"
            | "text/css"
            | "application/json"
            | "application/manifest+json"
            | "image/svg+xml"
    )
}

/// Compression predicate for the layer in [`static_assets`]: a 200 whose
/// media type [`is_compressible`]. A 304 carries no body and no media type.
fn should_compress(
    status: StatusCode,
    _version: Version,
    headers: &HeaderMap,
    _extensions: &Extensions,
) -> bool {
    status == StatusCode::OK
        && headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|essence| is_compressible(essence.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    /// A GET of `path` through the router, with the given request headers.
    async fn get(path: &str, request_headers: &[(&str, &str)]) -> Response {
        let mut request = Request::builder().method(Method::GET).uri(path);
        for (name, value) in request_headers {
            request = request.header(*name, *value);
        }
        static_assets()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_of(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    fn header_of<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
        resp.headers()
            .get(name)
            .map(|value| value.to_str().unwrap())
    }

    /// The path of an embedded hashed build output ending in `suffix`.
    fn hashed_asset(suffix: &str) -> String {
        WebAssets::iter()
            .map(std::borrow::Cow::into_owned)
            .find(|path| path.starts_with(HASHED_ASSET_DIR) && path.ends_with(suffix))
            .unwrap()
    }

    fn embedded_bytes(path: &str) -> Vec<u8> {
        WebAssets::get(path).unwrap().data.to_vec()
    }

    #[test]
    fn web_assets_contains_index_html() {
        assert!(
            WebAssets::get("index.html").is_some(),
            "index.html should be embedded"
        );
    }

    #[test]
    fn serve_embedded_returns_none_for_missing() {
        assert!(
            serve_embedded("does-not-exist.txt", &Method::GET, &HeaderMap::new()).is_none(),
            "missing file should return None"
        );
    }

    #[tokio::test]
    async fn content_types_match_the_file() {
        for (path, expected) in [
            ("/index.html", "text/html"),
            ("/mcp-catalog.json", "application/json"),
            ("/manifest.webmanifest", "application/manifest+json"),
            ("/favicon.svg", "image/svg+xml"),
            ("/icons/icon-192.png", "image/png"),
        ] {
            let resp = get(path, &[]).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert_eq!(header_of(&resp, "content-type"), Some(expected), "{path}");
        }
    }

    #[tokio::test]
    async fn hashed_assets_are_cached_for_a_year_without_an_etag() {
        for suffix in [".js", ".css"] {
            let path = format!("/{}", hashed_asset(suffix));
            let resp = get(&path, &[]).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert_eq!(
                header_of(&resp, "cache-control"),
                Some("public, max-age=31536000, immutable"),
                "{path}"
            );
            assert!(resp.headers().get("etag").is_none(), "{path} has no etag");
        }
    }

    #[tokio::test]
    async fn non_html_embedded_files_revalidate_against_a_strong_etag() {
        for path in [
            "/manifest.webmanifest",
            "/favicon.svg",
            "/mcp-catalog.json",
            "/icons/icon-192.png",
        ] {
            let resp = get(path, &[]).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert_eq!(
                header_of(&resp, "cache-control"),
                Some("no-cache"),
                "{path}"
            );
            let etag = header_of(&resp, "etag").unwrap();
            assert!(
                etag.starts_with('"') && etag.ends_with('"') && etag.len() > 2,
                "{path} etag {etag} is a quoted strong validator"
            );
        }
    }

    #[tokio::test]
    async fn etag_identifies_the_content() {
        let manifest = get("/manifest.webmanifest", &[]).await;
        let again = get("/manifest.webmanifest", &[]).await;
        let catalog = get("/mcp-catalog.json", &[]).await;
        assert_eq!(
            header_of(&manifest, "etag"),
            header_of(&again, "etag"),
            "the same content has the same etag"
        );
        assert_ne!(
            header_of(&manifest, "etag"),
            header_of(&catalog, "etag"),
            "different content has a different etag"
        );
    }

    #[tokio::test]
    async fn html_documents_are_always_fetched_whole() {
        let index_body = embedded_bytes("index.html");

        for path in [
            "/index.html",
            "/",
            "/agent/atlas/files",
            "/sessions/run-1790000000000-0a1b2c3d",
            "/settings/memory",
        ] {
            let resp = get(path, &[]).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path} should be served");
            assert_eq!(
                header_of(&resp, "content-type"),
                Some("text/html"),
                "{path}"
            );
            assert_eq!(
                header_of(&resp, "cache-control"),
                Some("no-cache"),
                "{path}"
            );
            assert!(resp.headers().get("etag").is_none(), "{path} has no etag");
            assert_eq!(body_of(resp).await, index_body, "{path} is index.html");
        }
    }

    #[tokio::test]
    async fn html_documents_never_answer_304() {
        let index_body = embedded_bytes("index.html");

        for path in ["/index.html", "/", "/agent/atlas/files"] {
            for validator in ["*", "\"anything\"", "W/\"anything\""] {
                let resp = get(path, &[("if-none-match", validator)]).await;
                assert_eq!(
                    resp.status(),
                    StatusCode::OK,
                    "{path} with If-None-Match {validator}"
                );
                assert_eq!(body_of(resp).await, index_body, "{path}");
            }
        }
    }

    #[tokio::test]
    async fn missing_endpoints_and_assets_404_without_the_app_shell() {
        for path in [
            "/api/nope",
            "/api",
            "/ws/extra",
            "/assets/missing-abc123.js",
            "/does-not-exist.txt",
        ] {
            let resp = get(path, &[]).await;
            assert_eq!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{path} should not get the app shell"
            );
            assert!(resp.headers().get("etag").is_none(), "{path}");
            assert!(resp.headers().get("content-encoding").is_none(), "{path}");
        }
    }

    #[tokio::test]
    async fn matching_if_none_match_gets_a_bodyless_304() {
        for path in ["/manifest.webmanifest", "/icons/icon-192.png"] {
            let etag = header_of(&get(path, &[]).await, "etag")
                .unwrap()
                .to_string();
            let resp = get(path, &[("if-none-match", &etag)]).await;
            assert_eq!(resp.status(), StatusCode::NOT_MODIFIED, "{path}");
            assert_eq!(header_of(&resp, "etag"), Some(etag.as_str()), "{path}");
            assert_eq!(
                header_of(&resp, "cache-control"),
                Some("no-cache"),
                "{path}"
            );
            assert!(body_of(resp).await.is_empty(), "{path} 304 has no body");
        }
    }

    #[tokio::test]
    async fn mismatched_if_none_match_gets_the_file() {
        let resp = get(
            "/manifest.webmanifest",
            &[("if-none-match", "\"not-the-etag\"")],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            body_of(resp).await,
            embedded_bytes("manifest.webmanifest"),
            "a stale validator gets the current body"
        );
    }

    #[tokio::test]
    async fn if_none_match_accepts_lists_weak_validators_and_star() {
        let path = "/manifest.webmanifest";
        let etag = header_of(&get(path, &[]).await, "etag")
            .unwrap()
            .to_string();
        for value in [
            format!("\"other\", {etag}"),
            format!("W/{etag}"),
            format!("  {etag}  "),
            "*".to_string(),
        ] {
            let resp = get(path, &[("if-none-match", &value)]).await;
            assert_eq!(resp.status(), StatusCode::NOT_MODIFIED, "{value}");
        }
    }

    #[tokio::test]
    async fn only_get_and_head_are_answered_with_304() {
        let etag = header_of(&get("/manifest.webmanifest", &[]).await, "etag")
            .unwrap()
            .to_string();
        let with_method = |method: Method| {
            Request::builder()
                .method(method)
                .uri("/manifest.webmanifest")
                .header("if-none-match", &etag)
                .body(Body::empty())
                .unwrap()
        };
        let post = static_assets()
            .oneshot(with_method(Method::POST))
            .await
            .unwrap();
        assert_eq!(post.status(), StatusCode::OK);
        let head = static_assets()
            .oneshot(with_method(Method::HEAD))
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn script_is_compressed_only_when_the_client_accepts_it() {
        let path = format!("/{}", hashed_asset(".js"));
        let original = embedded_bytes(path.trim_start_matches('/'));

        let unasked = get(&path, &[]).await;
        assert_eq!(header_of(&unasked, "content-encoding"), None);
        assert_eq!(
            header_of(&unasked, "vary"),
            Some("Accept-Encoding"),
            "an uncompressed answer still varies on the encoding"
        );
        assert_eq!(body_of(unasked).await, original);

        let identity = get(&path, &[("accept-encoding", "identity")]).await;
        assert_eq!(header_of(&identity, "content-encoding"), None);
        assert_eq!(body_of(identity).await, original);

        let gzip = get(&path, &[("accept-encoding", "gzip")]).await;
        assert_eq!(header_of(&gzip, "content-encoding"), Some("gzip"));
        assert_eq!(header_of(&gzip, "vary"), Some("Accept-Encoding"));
        assert_eq!(
            header_of(&gzip, "cache-control"),
            Some("public, max-age=31536000, immutable")
        );
        let gzipped = body_of(gzip).await;
        assert_eq!(
            gzipped.get(..2),
            Some([0x1f, 0x8b].as_slice()),
            "gzip magic"
        );
        assert!(gzipped.len() < original.len(), "gzip shrinks the bundle");

        let brotli = get(&path, &[("accept-encoding", "br")]).await;
        assert_eq!(header_of(&brotli, "content-encoding"), Some("br"));
        assert_eq!(header_of(&brotli, "vary"), Some("Accept-Encoding"));
        let brotlied = body_of(brotli).await;
        assert_ne!(brotlied, original);
        assert!(brotlied.len() < original.len(), "brotli shrinks the bundle");
    }

    #[tokio::test]
    async fn a_browsers_usual_accept_encoding_gets_brotli_or_gzip() {
        let path = format!("/{}", hashed_asset(".js"));
        let resp = get(&path, &[("accept-encoding", "gzip, deflate, br, zstd")]).await;
        assert!(
            matches!(header_of(&resp, "content-encoding"), Some("br" | "gzip")),
            "got {:?}",
            header_of(&resp, "content-encoding")
        );
    }

    #[tokio::test]
    async fn styles_json_svg_and_manifest_are_compressed() {
        let css = format!("/{}", hashed_asset(".css"));
        for path in [
            css.as_str(),
            "/mcp-catalog.json",
            "/favicon.svg",
            "/manifest.webmanifest",
        ] {
            let resp = get(path, &[("accept-encoding", "br, gzip")]).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert!(
                resp.headers().get("content-encoding").is_some(),
                "{path} should be compressed"
            );
            assert_eq!(header_of(&resp, "vary"), Some("Accept-Encoding"), "{path}");
        }
    }

    #[tokio::test]
    async fn html_is_never_compressed() {
        let original = embedded_bytes("index.html");
        for path in ["/index.html", "/", "/agent/atlas/files"] {
            let resp = get(path, &[("accept-encoding", "gzip, deflate, br, zstd")]).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert_eq!(header_of(&resp, "content-encoding"), None, "{path}");
            assert_eq!(header_of(&resp, "vary"), None, "{path}");
            assert_eq!(body_of(resp).await, original, "{path} is the plain page");
        }
    }

    #[tokio::test]
    async fn images_are_not_compressed() {
        let resp = get("/icons/icon-192.png", &[("accept-encoding", "br, gzip")]).await;
        assert_eq!(header_of(&resp, "content-encoding"), None);
        assert_eq!(header_of(&resp, "vary"), None);
    }

    #[tokio::test]
    async fn not_modified_repeats_vary_and_is_not_compressed() {
        let path = "/manifest.webmanifest";
        let etag = header_of(&get(path, &[]).await, "etag")
            .unwrap()
            .to_string();
        let resp = get(
            path,
            &[("if-none-match", &etag), ("accept-encoding", "br, gzip")],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header_of(&resp, "vary"), Some("Accept-Encoding"));
        assert_eq!(header_of(&resp, "content-encoding"), None);
        assert!(body_of(resp).await.is_empty());
    }

    #[tokio::test]
    async fn etag_is_the_same_for_every_encoding() {
        let path = "/manifest.webmanifest";
        let plain = get(path, &[]).await;
        let gzip = get(path, &[("accept-encoding", "gzip")]).await;
        assert_eq!(header_of(&plain, "etag"), header_of(&gzip, "etag"));
    }

    /// The Residuum Cloud tunnel forwards requests with a `reqwest` client
    /// that doesn't decompress, and hands the relay the raw body and headers.
    #[tokio::test]
    async fn a_client_that_does_not_decompress_receives_the_encoded_body_and_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, static_assets()).await.ok();
        });
        let client = reqwest::Client::new();
        let path = hashed_asset(".js");
        let url = format!("http://{addr}/{path}");

        let script = client
            .get(&url)
            .header("accept-encoding", "gzip")
            .send()
            .await
            .unwrap();
        assert_eq!(script.status(), reqwest::StatusCode::OK);
        assert_eq!(script.headers()["content-encoding"], "gzip");
        assert_eq!(script.headers()["vary"], "Accept-Encoding");
        assert_eq!(
            script.headers()["cache-control"],
            "public, max-age=31536000, immutable"
        );
        let body = script.bytes().await.unwrap();
        assert_eq!(body.get(..2), Some([0x1f, 0x8b].as_slice()));

        let manifest_url = format!("http://{addr}/manifest.webmanifest");
        let manifest = client.get(&manifest_url).send().await.unwrap();
        let etag = manifest.headers()["etag"].to_str().unwrap().to_string();
        let revalidated = client
            .get(&manifest_url)
            .header("if-none-match", &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(revalidated.status(), reqwest::StatusCode::NOT_MODIFIED);
        assert!(revalidated.bytes().await.unwrap().is_empty());

        let page_url = format!("http://{addr}/");
        let page = client
            .get(&page_url)
            .header("if-none-match", "*")
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), reqwest::StatusCode::OK);
        assert_eq!(page.bytes().await.unwrap(), embedded_bytes("index.html"));
    }

    #[test]
    fn only_text_html_is_an_html_document() {
        assert!(is_html("text/html"));
        for essence in [
            "application/json",
            "image/svg+xml",
            "text/css",
            "text/plain",
        ] {
            assert!(!is_html(essence), "{essence}");
        }
    }

    #[test]
    fn compressible_types_are_text_formats_the_app_ships() {
        for essence in [
            "text/javascript",
            "application/javascript",
            "text/css",
            "application/json",
            "application/manifest+json",
            "image/svg+xml",
        ] {
            assert!(is_compressible(essence), "{essence}");
        }
        for essence in [
            "text/html",
            "image/png",
            "application/octet-stream",
            "text/event-stream",
        ] {
            assert!(!is_compressible(essence), "{essence}");
        }
    }

    #[test]
    fn should_compress_reads_the_media_type_and_status() {
        let json = {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            );
            headers
        };
        let html = {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
            headers
        };
        let ext = Extensions::new();
        assert!(should_compress(
            StatusCode::OK,
            Version::HTTP_11,
            &json,
            &ext
        ));
        assert!(!should_compress(
            StatusCode::OK,
            Version::HTTP_11,
            &html,
            &ext
        ));
        assert!(
            !should_compress(StatusCode::NOT_MODIFIED, Version::HTTP_11, &json, &ext),
            "a 304 has no body to compress"
        );
        assert!(
            !should_compress(StatusCode::OK, Version::HTTP_11, &HeaderMap::new(), &ext),
            "no media type, no compression"
        );
    }
}
