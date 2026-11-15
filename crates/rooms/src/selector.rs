//! System 1 (`Question::Choice`) speaker selection for meeting rooms.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use residuum_inference::system_one::{Answer, Question, SystemOneError, SystemOneService};

use crate::types::{RoomMessage, RoomSeat, RoomSeatKind};

/// The question key sent to System 1 for room speaker selection.
pub const SPEAKER_QUESTION_KEY: &str = "next_speaker";

/// The option key meaning no agent should reply right now.
pub const NO_SPEAKER_OPTION: &str = "no_speaker";

/// Maximum number of recent transcript messages included in the selector state.
const RECENT_MESSAGE_LIMIT: usize = 12;

/// Decision outcome from [`SpeakerSelector::select_next`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorOutcome {
    /// An eligible `seat_id` was chosen to speak next.
    Pick(String),
    /// No agent needs to speak right now.
    NoSpeaker,
    /// Decision Model is unconfigured.
    Unavailable,
    /// Decision Model failed or rejected speaker selection for this request.
    Failed(String),
}

/// Abstracts speaker selection for [`crate::RoomService`] so unit tests can
/// drive deterministic selections without network calls.
#[async_trait]
pub trait SpeakerSelector: Send + Sync {
    /// Choose the next eligible seat to speak, [`SelectorOutcome::NoSpeaker`],
    /// or [`SelectorOutcome::Unavailable`].
    async fn select_next(
        &self,
        room_name: &str,
        room_topic: Option<&str>,
        recent_messages: &[RoomMessage],
        eligible_seats: &[RoomSeat],
    ) -> SelectorOutcome;

    /// Whether the underlying Decision Model is currently configured and healthy.
    fn is_available(&self) -> bool;
}

/// Speaker selector backed by Residuum's shared [`SystemOneService`].
#[derive(Clone)]
pub struct SystemOneSpeakerSelector {
    system_one: Arc<SystemOneService>,
}

impl SystemOneSpeakerSelector {
    /// Create a selector wrapping `system_one`.
    #[must_use]
    pub fn new(system_one: Arc<SystemOneService>) -> Self {
        Self { system_one }
    }
}

pub(crate) fn build_choice_request(
    room_name: &str,
    room_topic: Option<&str>,
    recent_messages: &[RoomMessage],
    eligible_seats: &[RoomSeat],
) -> (Value, BTreeMap<String, Question>) {
    let start = recent_messages.len().saturating_sub(RECENT_MESSAGE_LIMIT);
    let recent: Vec<Value> = recent_messages
        .get(start..)
        .unwrap_or_default()
        .iter()
        .map(|msg| {
            json!({
                "speaker": msg.author_name,
                "text": msg.content,
            })
        })
        .collect();
    let state = json!({
        "room_name": room_name,
        "room_topic": room_topic,
        "recent_messages": recent,
    });

    let mut criteria: BTreeMap<String, Option<Value>> = BTreeMap::new();
    criteria.insert(
        NO_SPEAKER_OPTION.to_owned(),
        Some(Value::String(
            "No agent needs to speak next (the message needs no reply, is purely acknowledgments, \
             or the discussion has naturally concluded)."
                .to_owned(),
        )),
    );
    for seat in eligible_seats {
        let kind_label = match seat.kind {
            RoomSeatKind::Local => "local",
            RoomSeatKind::SiblingA2a => "sibling_a2a",
            RoomSeatKind::ExternalA2a => "external_a2a",
        };
        let role = seat
            .role
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .unwrap_or("No role description provided");
        criteria.insert(
            seat.seat_id.clone(),
            Some(Value::String(format!(
                "{} ({kind_label}): {role}",
                seat.display_name
            ))),
        );
    }

    let question = Question::Choice {
        instructions: Value::String(
            "Select the single participant best suited to reply next to the conversation in \
             state.recent_messages based on each option's role description, or select \
             \"no_speaker\" if no agent response is needed."
                .to_owned(),
        ),
        criteria,
    };
    let questions = BTreeMap::from([(SPEAKER_QUESTION_KEY.to_owned(), question)]);
    (state, questions)
}

#[async_trait]
impl SpeakerSelector for SystemOneSpeakerSelector {
    async fn select_next(
        &self,
        room_name: &str,
        room_topic: Option<&str>,
        recent_messages: &[RoomMessage],
        eligible_seats: &[RoomSeat],
    ) -> SelectorOutcome {
        if !self.system_one.status().configured {
            return SelectorOutcome::Unavailable;
        }
        if eligible_seats.is_empty() {
            return SelectorOutcome::NoSpeaker;
        }
        let (state, questions) =
            build_choice_request(room_name, room_topic, recent_messages, eligible_seats);
        match self.system_one.evaluate(&state, &questions).await {
            Ok(resp) => match resp.answers.get(SPEAKER_QUESTION_KEY) {
                Some(Answer::Choice { choice, .. }) => {
                    if choice == NO_SPEAKER_OPTION {
                        SelectorOutcome::NoSpeaker
                    } else if eligible_seats.iter().any(|s| s.seat_id == *choice) {
                        SelectorOutcome::Pick(choice.clone())
                    } else {
                        tracing::warn!(
                            choice = %choice,
                            "system one returned unknown or ineligible room speaker; treating as no_speaker"
                        );
                        SelectorOutcome::NoSpeaker
                    }
                }
                other => {
                    tracing::warn!(
                        answer = ?other,
                        "system one response missing choice answer for room speaker selection"
                    );
                    SelectorOutcome::Failed(
                        "Decision model returned an unreadable speaker choice.".to_owned(),
                    )
                }
            },
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "system one room speaker selection failed; entering mention-only fallback"
                );
                if matches!(err, SystemOneError::NotConfigured) {
                    SelectorOutcome::Unavailable
                } else {
                    SelectorOutcome::Failed(format_selector_failure(&err))
                }
            }
        }
    }

    fn is_available(&self) -> bool {
        let status = self.system_one.status();
        status.configured && status.outage.is_none()
    }
}

const MAX_SELECTOR_ERROR_CHARS: usize = 220;

fn format_selector_failure(err: &SystemOneError) -> String {
    if let SystemOneError::RequestRejected { provider, detail } = err {
        let cleaned = extract_error_detail(detail);
        if cleaned.is_empty() {
            format!("{provider} rejected speaker selection for this message.")
        } else {
            truncate_chars(
                &format!("{provider} rejected speaker selection: {cleaned}"),
                MAX_SELECTOR_ERROR_CHARS,
            )
        }
    } else {
        truncate_chars(&err.user_message(), MAX_SELECTOR_ERROR_CHARS)
    }
}

fn extract_error_detail(detail: &str) -> String {
    let trimmed = detail.trim();
    let json_candidate = trimmed
        .find('{')
        .and_then(|idx| trimmed.get(idx..))
        .unwrap_or(trimmed);
    if let Ok(val) = serde_json::from_str::<Value>(json_candidate)
        && let Some(msg) = val
            .get("error")
            .and_then(|e| e.get("message").or(Some(e)))
            .and_then(Value::as_str)
    {
        if let Some(inner_start) = msg.find('{')
            && let Some(inner_tail) = msg.get(inner_start..)
        {
            let inner_slice = inner_tail
                .rfind('}')
                .and_then(|end| inner_tail.get(..=end))
                .unwrap_or(inner_tail);
            if let Ok(inner_val) = serde_json::from_str::<Value>(inner_slice)
                && let Some(inner_msg) = inner_val
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
            {
                return inner_msg.trim().to_owned();
            }
        }
        let first_line = msg.lines().next().unwrap_or(msg).trim();
        if !first_line.is_empty() {
            return first_line.to_owned();
        }
    }
    trimmed.lines().next().unwrap_or("").trim().to_owned()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use residuum_config::{SystemOneConfig, SystemOneProvider};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::types::{RoomId, RoomMessageKind};

    fn sample_seats() -> Vec<RoomSeat> {
        vec![
            RoomSeat {
                seat_id: "agent:scout".to_owned(),
                kind: RoomSeatKind::Local,
                display_name: "Scout".to_owned(),
                role: Some("Research and fact finding".to_owned()),
                instance: None,
            },
            RoomSeat {
                seat_id: "a2a:partner".to_owned(),
                kind: RoomSeatKind::ExternalA2a,
                display_name: "Partner".to_owned(),
                role: None,
                instance: None,
            },
        ]
    }

    fn sample_message(seq: u64, author: &str, text: &str) -> RoomMessage {
        RoomMessage {
            seq,
            id: format!("rmsg_{seq:014}"),
            room_id: RoomId::parse("room_01jcv8k9m2p4q7").unwrap(),
            kind: RoomMessageKind::Owner,
            seat_id: None,
            author_name: author.to_owned(),
            seat_kind: None,
            content: text.to_owned(),
            attachments: Vec::new(),
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn unconfigured_system_one_returns_unavailable() {
        let service = SystemOneService::new(None);
        let selector = SystemOneSpeakerSelector::new(service);
        assert!(
            !selector.is_available(),
            "unconfigured selector must report unavailable"
        );
        let outcome = selector
            .select_next(
                "Design Sync",
                Some("Scope"),
                &[sample_message(1, "Owner", "Thoughts?")],
                &sample_seats(),
            )
            .await;
        assert_eq!(
            outcome,
            SelectorOutcome::Unavailable,
            "unconfigured SystemOne must return Unavailable"
        );
    }

    #[test]
    fn build_choice_request_caps_recent_messages_and_formats_criteria() {
        let messages: Vec<RoomMessage> = (1_u64..=15)
            .map(|i| sample_message(i, "Owner", &format!("msg {i}")))
            .collect();
        let seats = sample_seats();
        let (state, questions) =
            build_choice_request("Standup", Some("Blockers"), &messages, &seats);
        let recent = state
            .get("recent_messages")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(recent.len(), 12, "recent_messages must cap at 12 entries");
        assert_eq!(
            recent
                .first()
                .and_then(|v| v.get("text"))
                .and_then(Value::as_str),
            Some("msg 4"),
            "oldest retained message must be seq 4"
        );
        let Some(Question::Choice { criteria, .. }) = questions.get(SPEAKER_QUESTION_KEY) else {
            panic!("expected Question::Choice under {SPEAKER_QUESTION_KEY}");
        };
        assert!(
            criteria.contains_key(NO_SPEAKER_OPTION),
            "criteria must include no_speaker"
        );
        assert_eq!(
            criteria
                .get("agent:scout")
                .and_then(|v| v.as_ref())
                .and_then(Value::as_str),
            Some("Scout (local): Research and fact finding"),
            "local seat criterion must match contract format"
        );
        assert_eq!(
            criteria
                .get("a2a:partner")
                .and_then(|v| v.as_ref())
                .and_then(Value::as_str),
            Some("Partner (external_a2a): No role description provided"),
            "external seat without role must get fallback role text"
        );
    }

    #[tokio::test]
    async fn system_one_selector_picks_seat_no_speaker_and_ignores_ineligible() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "decision-v1",
                "answers": {
                    "next_speaker": {
                        "type": "choice",
                        "choice": "agent:scout"
                    }
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "decision-v1",
                "answers": {
                    "next_speaker": {
                        "type": "choice",
                        "choice": "no_speaker"
                    }
                }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "decision-v1",
                "answers": {
                    "next_speaker": {
                        "type": "choice",
                        "choice": "agent:ineligible"
                    }
                }
            })))
            .mount(&server)
            .await;

        let cfg = SystemOneConfig {
            provider: SystemOneProvider::TypeSafe,
            url: server.uri(),
            model: "decision-v1".to_owned(),
            api_key: None,
            keep_alive: None,
        };
        let selector = SystemOneSpeakerSelector::new(SystemOneService::new(Some(&cfg)));
        assert!(
            selector.is_available(),
            "configured selector must be available"
        );
        let seats = sample_seats();
        let msgs = [sample_message(1, "Owner", "Who knows?")];

        assert_eq!(
            selector.select_next("Sync", None, &msgs, &seats).await,
            SelectorOutcome::Pick("agent:scout".to_owned()),
            "eligible choice must return Pick"
        );
        assert_eq!(
            selector.select_next("Sync", None, &msgs, &seats).await,
            SelectorOutcome::NoSpeaker,
            "no_speaker must return NoSpeaker"
        );
        assert_eq!(
            selector.select_next("Sync", None, &msgs, &seats).await,
            SelectorOutcome::NoSpeaker,
            "ineligible seat choice must safely degrade to NoSpeaker"
        );
    }

    #[tokio::test]
    async fn system_one_selector_surfaces_clean_failure_reason_on_400_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "message": "proxy error - {\"error\":{\"message\":\"Input blocked: prompt contains sensitive words.\",\"code\":\"prohibited_content\"}}\n\nNo fallback configured."
                }
            })))
            .mount(&server)
            .await;

        let cfg = SystemOneConfig {
            provider: SystemOneProvider::Other,
            url: server.uri(),
            model: "decision-v1".to_owned(),
            api_key: None,
            keep_alive: None,
        };
        let selector = SystemOneSpeakerSelector::new(SystemOneService::new(Some(&cfg)));
        let seats = sample_seats();
        let msgs = [sample_message(1, "Owner", "Sensitive prompt")];

        let outcome = selector.select_next("Sync", None, &msgs, &seats).await;
        let SelectorOutcome::Failed(reason) = outcome else {
            panic!("400 rejection must return SelectorOutcome::Failed, got {outcome:?}");
        };
        assert!(
            reason.contains("rejected speaker selection"),
            "failure reason must explain rejection: {reason}"
        );
        assert!(
            reason.contains("Input blocked: prompt contains sensitive words."),
            "failure reason must unwrap nested provider message: {reason}"
        );
        assert!(
            selector.is_available(),
            "per-request 400 rejection must keep global selector available"
        );
    }
}
