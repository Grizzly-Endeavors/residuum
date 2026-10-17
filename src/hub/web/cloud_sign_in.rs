//! Sign-ins to Residuum Cloud that this install started.
//!
//! `/cloud/callback` stores the token it is handed and switches the tunnel on,
//! and it is a plain `GET` that any web page can send the browser to. Without
//! something tying the callback to a Connect in this app, a page the person
//! visits could sign the install in to the page owner's account. So Connect
//! asks the hub for the sign-in address (`POST /api/hub/cloud/connect`), the
//! hub puts a random `state` in it and remembers it, the relay carries `state`
//! through its sign-in page and back, and the callback takes a token only
//! together with a `state` the hub handed out, once, and recently.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use super::cloud::CloudApiState;
use crate::util::secrets_match;

/// How long a started sign-in can take to come back. Signing in on the relay
/// can include creating an account, so this is generous.
const SIGN_IN_LIFETIME: Duration = Duration::from_mins(15);

/// How many started sign-ins the hub remembers at once. Someone who clicks
/// Connect a few times and finishes in any of the tabs it opened still gets
/// in; the oldest is forgotten when a ninth starts.
const MAX_PENDING_SIGN_INS: usize = 8;

/// Random bytes in a `state`: 256 bits, 43 characters of URL-safe base64,
/// inside the relay's `^[A-Za-z0-9_-]{16,128}$`.
const STATE_BYTES: usize = 32;

/// The sign-ins Connect started that haven't come back yet, newest last.
/// They live in memory only: a hub restart forgets them, and a sign-in
/// started before it is refused like any other unknown one.
#[derive(Clone, Default)]
pub(crate) struct PendingSignIns {
    started: Arc<Mutex<VecDeque<StartedSignIn>>>,
}

struct StartedSignIn {
    state: String,
    at: Instant,
}

/// What a callback's `state` says about where the sign-in came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignInOrigin {
    /// A sign-in Connect started here, within its lifetime. It is used up.
    StartedHere,
    /// A sign-in Connect started here, but longer ago than its lifetime.
    Expired,
    /// No sign-in this hub remembers: never handed out, already used, or
    /// pushed out by newer ones.
    Unknown,
    /// The callback carried no `state`.
    Missing,
}

impl PendingSignIns {
    /// Start a sign-in: a fresh `state`, remembered until it comes back.
    pub(super) fn start(&self) -> Result<String, ring::error::Unspecified> {
        let mut bytes = [0_u8; STATE_BYTES];
        SystemRandom::new().fill(&mut bytes)?;
        let state = URL_SAFE_NO_PAD.encode(bytes);
        let now = Instant::now();
        let mut started = self.lock();
        started.retain(|sign_in| now.duration_since(sign_in.at) < SIGN_IN_LIFETIME);
        while started.len() >= MAX_PENDING_SIGN_INS {
            started.pop_front();
        }
        started.push_back(StartedSignIn {
            state: state.clone(),
            at: now,
        });
        Ok(state)
    }

    /// Use up the started sign-in `presented` names, if there is one. Every
    /// remembered `state` is compared, each in constant time, so the answer's
    /// timing says nothing about how close a guess came.
    fn redeem(&self, presented: &str) -> SignInOrigin {
        let mut started = self.lock();
        let mut found = None;
        for (index, sign_in) in started.iter().enumerate() {
            if secrets_match(presented, &sign_in.state) {
                found = Some(index);
            }
        }
        match found.and_then(|index| started.remove(index)) {
            Some(sign_in) if sign_in.at.elapsed() < SIGN_IN_LIFETIME => SignInOrigin::StartedHere,
            Some(_) => SignInOrigin::Expired,
            None => SignInOrigin::Unknown,
        }
    }

    /// Whether the callback query `query` comes back from a sign-in Connect
    /// started here, using that sign-in up. Logs why at `warn` when it
    /// doesn't; the token is never logged.
    pub(crate) fn callback_started_here(&self, query: Option<&str>) -> bool {
        let origin = match state_from_query(query) {
            Some(presented) => self.redeem(&presented),
            None => SignInOrigin::Missing,
        };
        let why = match origin {
            SignInOrigin::StartedHere => return true,
            SignInOrigin::Expired => "its state is from a sign-in started too long ago",
            SignInOrigin::Unknown => {
                "its state names no sign-in this install has open (never started here, already used, or replaced by newer ones)"
            }
            SignInOrigin::Missing => "it carries no state",
        };
        tracing::warn!(
            reason = why,
            "refused a cloud sign-in callback that didn't come from a Connect in this app; nothing was saved"
        );
        false
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<StartedSignIn>> {
        self.started.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The first non-empty `state` in a raw callback query.
fn state_from_query(query: Option<&str>) -> Option<String> {
    url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(key, value)| key == "state" && !value.is_empty())
        .map(|(_, value)| value.into_owned())
}

/// The relay the sign-in is for, as the Settings form has it.
#[derive(Deserialize)]
pub(crate) struct ConnectRequest {
    /// The relay URL the tunnel would dial; empty means Residuum Cloud's.
    #[serde(default)]
    relay_url: String,
    /// The gateway port the relay sends the browser back to; empty means
    /// the default.
    #[serde(default)]
    port: String,
}

/// Where to send the browser to sign in.
#[derive(Serialize)]
pub(crate) struct ConnectResponse {
    url: String,
}

/// `POST /api/hub/cloud/connect` — start a sign-in and answer with the relay's
/// sign-in address for it, `<relay origin>/connect?port=<port>&state=<state>`.
pub(crate) async fn api_cloud_connect(
    State(state): State<CloudApiState>,
    Json(request): Json<ConnectRequest>,
) -> Response {
    let Some(origin) = relay_web_origin(&request.relay_url) else {
        return connect_refused(
            StatusCode::BAD_REQUEST,
            "The relay address isn't a ws:// or wss:// address, so there is nowhere to sign in. Check the relay URL under More options.",
        );
    };
    let port = match request.port.trim() {
        "" => crate::config::DEFAULT_GATEWAY_PORT,
        given => match given.parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => {
                return connect_refused(
                    StatusCode::BAD_REQUEST,
                    "The gateway port isn't a port number, so the sign-in couldn't come back here. Check the port under General.",
                );
            }
        },
    };
    let sign_in_state = match state.sign_ins.start() {
        Ok(sign_in_state) => sign_in_state,
        Err(_unspecified) => {
            tracing::error!(
                "the operating system's random source failed while starting a cloud sign-in"
            );
            return connect_refused(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Residuum couldn't start a sign-in, because this computer's random number source failed. Try again; if it keeps failing, the details are in Residuum's logs.",
            );
        }
    };
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("port", &port.to_string())
        .append_pair("state", &sign_in_state)
        .finish();
    tracing::info!(relay = %origin, port, "started a cloud sign-in");
    Json(ConnectResponse {
        url: format!("{origin}/connect?{query}"),
    })
    .into_response()
}

fn connect_refused(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

/// The web origin of the relay the tunnel would dial: `ws` read as `http`,
/// `wss` as `https`, Residuum Cloud's when `relay_url` is empty. `None` when
/// it isn't a `ws://` or `wss://` address with a host.
fn relay_web_origin(relay_url: &str) -> Option<String> {
    let relay_url = match relay_url.trim() {
        "" => crate::config::DEFAULT_CLOUD_RELAY_URL,
        given => given,
    };
    let mut url = url::Url::parse(relay_url).ok()?;
    let web_scheme = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        _ => return None,
    };
    url.host_str()?;
    url.set_scheme(web_scheme).ok()?;
    Some(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY_STATE_PATTERN: &str = "^[A-Za-z0-9_-]{16,128}$";

    fn matches_relay_pattern(state: &str) -> bool {
        regex::Regex::new(RELAY_STATE_PATTERN)
            .unwrap()
            .is_match(state)
    }

    #[test]
    fn a_started_sign_in_is_fresh_and_fits_the_relays_pattern() {
        let sign_ins = PendingSignIns::default();
        let first = sign_ins.start().unwrap();
        let second = sign_ins.start().unwrap();
        assert!(matches_relay_pattern(&first), "{first}");
        assert!(matches_relay_pattern(&second), "{second}");
        assert_ne!(first, second);
    }

    #[test]
    fn a_started_sign_in_comes_back_once() {
        let sign_ins = PendingSignIns::default();
        let state = sign_ins.start().unwrap();
        assert_eq!(sign_ins.redeem(&state), SignInOrigin::StartedHere);
        assert_eq!(sign_ins.redeem(&state), SignInOrigin::Unknown);
    }

    #[test]
    fn a_state_never_handed_out_is_unknown() {
        let sign_ins = PendingSignIns::default();
        let state = sign_ins.start().unwrap();
        assert_eq!(
            sign_ins.redeem("not-a-state-this-hub-made"),
            SignInOrigin::Unknown
        );
        assert_eq!(sign_ins.redeem(""), SignInOrigin::Unknown);
        assert_eq!(sign_ins.redeem(&state), SignInOrigin::StartedHere);
    }

    #[test]
    fn any_of_several_started_sign_ins_comes_back_and_the_oldest_is_forgotten_past_the_limit() {
        let sign_ins = PendingSignIns::default();
        let mut states: VecDeque<String> = (0..=MAX_PENDING_SIGN_INS)
            .map(|_| sign_ins.start().unwrap())
            .collect();
        let oldest = states.pop_front().unwrap();
        assert_eq!(sign_ins.redeem(&oldest), SignInOrigin::Unknown);
        // Out of order: any of them may be the tab the person finishes in.
        states.rotate_left(3);
        for state in states {
            assert_eq!(sign_ins.redeem(&state), SignInOrigin::StartedHere);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_sign_in_expires_after_its_lifetime() {
        let sign_ins = PendingSignIns::default();
        let late = sign_ins.start().unwrap();
        let on_time = sign_ins.start().unwrap();
        residuum_core::testing::clock::elapse(
            SIGN_IN_LIFETIME
                .checked_sub(Duration::from_secs(1))
                .unwrap(),
        )
        .await;
        assert_eq!(sign_ins.redeem(&on_time), SignInOrigin::StartedHere);
        residuum_core::testing::clock::elapse(Duration::from_secs(1)).await;
        assert_eq!(sign_ins.redeem(&late), SignInOrigin::Expired);
        assert_eq!(sign_ins.redeem(&late), SignInOrigin::Unknown);
    }

    #[tokio::test(start_paused = true)]
    async fn starting_a_sign_in_forgets_the_expired_ones() {
        let sign_ins = PendingSignIns::default();
        let old = sign_ins.start().unwrap();
        residuum_core::testing::clock::elapse(SIGN_IN_LIFETIME).await;
        sign_ins.start().unwrap();
        assert_eq!(sign_ins.redeem(&old), SignInOrigin::Unknown);
    }

    #[test]
    fn the_callback_needs_the_state_of_a_sign_in_started_here() {
        let sign_ins = PendingSignIns::default();
        let state = sign_ins.start().unwrap();
        assert!(!sign_ins.callback_started_here(None));
        assert!(!sign_ins.callback_started_here(Some("token=rst_x")));
        assert!(!sign_ins.callback_started_here(Some("token=rst_x&state=")));
        assert!(!sign_ins.callback_started_here(Some("token=rst_x&state=forged_state_value")));
        assert!(sign_ins.callback_started_here(Some(&format!("token=rst_x&state={state}"))));
        assert!(!sign_ins.callback_started_here(Some(&format!("token=rst_x&state={state}"))));
    }

    #[test]
    fn the_relay_origin_comes_from_the_tunnel_address() {
        assert_eq!(
            relay_web_origin("").as_deref(),
            Some("https://agent-residuum.com")
        );
        assert_eq!(
            relay_web_origin(" wss://relay.example.com/tunnel/v2/register ").as_deref(),
            Some("https://relay.example.com")
        );
        assert_eq!(
            relay_web_origin("ws://127.0.0.1:8080/tunnel/v2/register").as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            relay_web_origin("WSS://Relay.Example.com:8443/x").as_deref(),
            Some("https://relay.example.com:8443")
        );
        assert_eq!(relay_web_origin("relay.example.com"), None);
        assert_eq!(relay_web_origin("https://relay.example.com"), None);
        assert_eq!(relay_web_origin("ws://"), None);
    }
}
