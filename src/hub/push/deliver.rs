//! One request to a push service, and what its answer means.

use std::time::Duration;

use super::encrypt::{Recipient, encrypt};
use super::keys::VapidKey;
use super::types::{PushMessage, WebPushSubscription};

/// How long one request may take, connecting and answering together.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// How much of a push service's answer is kept for the log.
const LOGGED_RESPONSE_BYTES: usize = 300;

/// Why a delivery didn't succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Failure {
    /// The HTTP status the push service answered, when it answered.
    pub(super) status: Option<u16>,
    /// What went wrong and what to do, in plain words.
    pub(super) message: String,
    /// What the push service said, for the log. It can hold anything, so it
    /// is never shown to the user.
    pub(super) response: Option<String>,
}

impl Failure {
    fn local(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
            response: None,
        }
    }
}

/// What one request to a push service came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Attempt {
    /// The push service accepted the message.
    Delivered,
    /// The push service no longer knows the subscription (404 or 410): the
    /// browser revoked it or it expired.
    Gone,
    /// A 429, a 5xx, or no answer at all: trying again later may work.
    Retryable(Failure),
    /// Anything else. The same request would fail the same way.
    Rejected(Failure),
}

/// Encrypt `message` for `subscription`, sign the request with `key` as
/// `subject`, and send it.
pub(super) async fn send_once(
    subscription: &WebPushSubscription,
    key: &VapidKey,
    subject: &str,
    message: &PushMessage,
    now_unix: i64,
) -> Attempt {
    let request = match build_request(subscription, key, subject, message, now_unix) {
        Ok(request) => request,
        Err(failure) => return Attempt::Rejected(failure),
    };
    let client = match reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        // A push service answers 201; following a redirect would send the
        // message to an address the subscription never named.
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            return Attempt::Rejected(Failure::local(format!(
                "Residuum couldn't prepare a connection to the push service: {}.",
                e.without_url()
            )));
        }
    };
    match client
        .post(request.url)
        .header("Authorization", request.authorization)
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("TTL", message.ttl.as_secs().to_string())
        .header("Urgency", message.urgency.header_value())
        .body(request.body)
        .send()
        .await
    {
        Ok(response) => classify(response).await,
        Err(e) => Attempt::Retryable(Failure::local(describe_transport(e))),
    }
}

/// Everything a request needs, before it is sent.
struct PreparedRequest {
    url: reqwest::Url,
    authorization: String,
    body: Vec<u8>,
}

fn build_request(
    subscription: &WebPushSubscription,
    key: &VapidKey,
    subject: &str,
    message: &PushMessage,
    now_unix: i64,
) -> Result<PreparedRequest, Failure> {
    let damaged = |what: &str| {
        Failure::local(format!(
            "This device's saved notification settings are damaged ({what}). Turn notifications \
             off and on again on that device."
        ))
    };
    let url = reqwest::Url::parse(&subscription.endpoint)
        .map_err(|e| damaged(&format!("its push address isn't a web address: {e}")))?;
    let recipient = Recipient::parse(&subscription.keys).map_err(|reason| damaged(&reason))?;
    let plaintext = serde_json::to_vec(&message.payload)
        .map_err(|e| Failure::local(format!("Residuum couldn't write the notification: {e}.")))?;
    let body = encrypt(&recipient, &plaintext).map_err(|e| {
        Failure::local(format!(
            "Residuum couldn't encrypt the notification: {e:#}."
        ))
    })?;
    let audience = url.origin().ascii_serialization();
    let authorization = key
        .authorization(&audience, subject, now_unix)
        .map_err(|e| Failure::local(format!("Residuum couldn't sign the request: {e:#}.")))?;
    Ok(PreparedRequest {
        url,
        authorization,
        body,
    })
}

/// Sort a push service's answer into what to do next.
async fn classify(response: reqwest::Response) -> Attempt {
    let status = response.status();
    if status.is_success() {
        return Attempt::Delivered;
    }
    let code = status.as_u16();
    if matches!(code, 404 | 410) {
        return Attempt::Gone;
    }
    let failure = Failure {
        status: Some(code),
        message: describe_status(code),
        response: first_bytes_of(response).await,
    };
    if code == 429 || status.is_server_error() {
        Attempt::Retryable(failure)
    } else {
        Attempt::Rejected(failure)
    }
}

/// The start of a response body, as text for the log.
async fn first_bytes_of(mut response: reqwest::Response) -> Option<String> {
    let chunk = response.chunk().await.ok().flatten()?;
    let kept: Vec<u8> = chunk.iter().take(LOGGED_RESPONSE_BYTES).copied().collect();
    let text = String::from_utf8_lossy(&kept);
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().filter(|c| !c.is_control()).collect())
}

fn describe_status(code: u16) -> String {
    match code {
        401 | 403 => format!(
            "The push service refused this notification (status {code}). Turn notifications off \
             and on again on that device."
        ),
        413 => format!("The notification was too large for the push service (status {code})."),
        429 => format!(
            "The push service is limiting how many notifications it accepts right now (status \
             {code})."
        ),
        500..=599 => format!("The push service had a problem on its side (status {code})."),
        _ => format!("The push service rejected the notification (status {code})."),
    }
}

fn describe_transport(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "The push service didn't answer in time.".to_string()
    } else if error.is_connect() {
        "Residuum couldn't reach the push service. Check this computer's internet connection."
            .to_string()
    } else {
        format!(
            "Residuum couldn't send to the push service: {}.",
            error.without_url()
        )
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::hub::push::encrypt::base64url_encode;
    use crate::hub::push::types::{PushEvent, PushPayload, WebPushSubscriptionKeys};

    async fn key() -> (tempfile::TempDir, VapidKey) {
        let dir = tempfile::tempdir().unwrap();
        let key = VapidKey::load_or_create(&dir.path().join("k"))
            .await
            .unwrap();
        (dir, key)
    }

    /// A subscription to `endpoint` whose keys are well-formed: the public
    /// key is the hub's own, which is a valid P-256 point.
    fn subscription(endpoint: &str, key: &VapidKey) -> WebPushSubscription {
        WebPushSubscription {
            endpoint: endpoint.to_string(),
            keys: WebPushSubscriptionKeys {
                p256dh: key.public_key_base64url(),
                auth: base64url_encode(&[7_u8; 16]),
            },
        }
    }

    fn message() -> PushMessage {
        PushMessage::new(PushPayload::new(
            PushEvent::AgentFailed,
            "scout",
            "scout couldn't start",
            "Its settings need fixing.",
            "/agent/scout",
            "failed:scout",
            2,
        ))
    }

    async fn attempt_against(status: u16) -> Attempt {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string("boom"))
            .mount(&server)
            .await;
        let (_dir, key) = key().await;
        let sub = subscription(&format!("{}/push/abc", server.uri()), &key);
        send_once(&sub, &key, "mailto:a@b.c", &message(), 1_000).await
    }

    #[tokio::test]
    async fn the_request_carries_the_push_headers_and_an_encrypted_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/push/abc"))
            .and(header("content-encoding", "aes128gcm"))
            .and(header("content-type", "application/octet-stream"))
            .and(header("ttl", "86400"))
            .and(header("urgency", "high"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        let (_dir, key) = key().await;
        let sub = subscription(&format!("{}/push/abc", server.uri()), &key);

        let attempt = send_once(&sub, &key, "mailto:a@b.c", &message(), 1_000).await;
        assert_eq!(attempt, Attempt::Delivered);

        let requests = server.received_requests().await.unwrap();
        let request = requests.first().unwrap();
        let authorization = request
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(authorization.starts_with("vapid t="), "{authorization}");
        assert!(
            authorization.ends_with(&format!("k={}", key.public_key_base64url())),
            "{authorization}"
        );
        assert_eq!(
            request.body.get(20..21),
            Some(&[65_u8][..]),
            "key id length"
        );
        assert!(
            !String::from_utf8_lossy(&request.body).contains("scout couldn't start"),
            "the body is encrypted"
        );
    }

    #[tokio::test]
    async fn the_token_names_the_endpoints_origin_and_the_contact() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        let (_dir, key) = key().await;
        let sub = subscription(&format!("{}/push/abc", server.uri()), &key);
        send_once(&sub, &key, "mailto:bear@example.com", &message(), 1_000).await;

        let requests = server.received_requests().await.unwrap();
        let authorization = requests
            .first()
            .unwrap()
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let jwt = authorization
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .map(|(jwt, _)| jwt.to_string())
            .unwrap();
        let claims = jwt.split('.').nth(1).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, claims)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            claims,
            serde_json::json!({
                "aud": server.uri(),
                "exp": 1_000 + 12 * 3600,
                "sub": "mailto:bear@example.com",
            })
        );
    }

    #[tokio::test]
    async fn success_statuses_deliver() {
        for status in [200, 201, 202] {
            assert_eq!(
                attempt_against(status).await,
                Attempt::Delivered,
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn an_expired_subscription_is_gone() {
        for status in [404, 410] {
            assert_eq!(attempt_against(status).await, Attempt::Gone, "{status}");
        }
    }

    #[tokio::test]
    async fn rate_limits_and_server_errors_are_worth_retrying() {
        for status in [429, 500, 502, 503] {
            let Attempt::Retryable(failure) = attempt_against(status).await else {
                panic!("{status} should be retryable");
            };
            assert_eq!(failure.status, Some(status));
            assert_eq!(failure.response.as_deref(), Some("boom"));
        }
    }

    #[tokio::test]
    async fn other_refusals_are_not_retried() {
        for status in [400, 401, 403, 413, 301] {
            let Attempt::Rejected(failure) = attempt_against(status).await else {
                panic!("{status} should be rejected");
            };
            assert_eq!(failure.status, Some(status));
            assert!(failure.message.contains(&status.to_string()), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/push/abc"))
            .respond_with(ResponseTemplate::new(307).insert_header("location", "/elsewhere"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/elsewhere"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;
        let (_dir, key) = key().await;
        let sub = subscription(&format!("{}/push/abc", server.uri()), &key);

        let attempt = send_once(&sub, &key, "mailto:a@b.c", &message(), 1_000).await;
        assert!(matches!(attempt, Attempt::Rejected(f) if f.status == Some(307)));
    }

    #[tokio::test]
    async fn an_unreachable_service_is_worth_retrying_and_the_message_hides_its_address() {
        let (_dir, key) = key().await;
        // Nothing listens on port 1.
        let sub = subscription("http://127.0.0.1:1/secret-capability-url", &key);

        let Attempt::Retryable(failure) =
            send_once(&sub, &key, "mailto:a@b.c", &message(), 1_000).await
        else {
            panic!("an unreachable service should be retryable");
        };
        assert_eq!(failure.status, None);
        assert!(
            !failure.message.contains("secret-capability-url"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn damaged_saved_settings_are_rejected_without_a_request() {
        let (_dir, key) = key().await;
        let mut sub = subscription("https://push.example/abc", &key);
        sub.keys.auth = "short".to_string();
        let Attempt::Rejected(failure) =
            send_once(&sub, &key, "mailto:a@b.c", &message(), 1_000).await
        else {
            panic!("damaged keys should be rejected");
        };
        assert_eq!(failure.status, None);
        assert!(failure.message.contains("off and on again"), "{failure:?}");

        let bad_address = subscription("not a url", &key);
        assert!(matches!(
            send_once(&bad_address, &key, "mailto:a@b.c", &message(), 1_000).await,
            Attempt::Rejected(_)
        ));
    }
}
