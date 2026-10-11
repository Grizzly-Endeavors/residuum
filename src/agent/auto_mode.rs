//! Auto Mode: plain-language rules a decision model checks every tool call
//! against before it runs.
//!
//! Each call becomes one System 1 request: the call (and the latest thing the
//! user asked for) is the state, and every rule is a yes/no question asked in
//! parallel. A call matching a deny rule, and no allow rule, is blocked; the
//! agent gets a tool result naming the rule and keeps going. When the decision
//! model can't answer, the call runs unchecked and the hub's System 1 status
//! (one Home notice) is the only signal.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_rs::TS;

use crate::config::AutoModeConfig;
use crate::inference::ToolCall;
use crate::inference::system_one::{Question, SystemOneResponse, SystemOneService};

/// Longest tool-call argument text sent to the decision model. Its context
/// is 32k tokens for the state plus the longest question; a call larger than
/// this is judged on its start.
const MAX_ARGUMENT_CHARS: usize = 8_000;

/// Longest user request sent alongside the call.
const MAX_USER_REQUEST_CHARS: usize = 2_000;

/// What Auto Mode decided about one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AutoModeDecision {
    /// Checked; no deny rule matched, or an allow rule overrode the match.
    Allowed,
    /// Checked; a deny rule matched and no allow rule did. The call didn't run.
    Blocked,
    /// The decision model couldn't answer, so the call ran without a check.
    Unchecked,
}

/// Auto Mode's verdict on one tool call, shown beside the call's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoModeVerdict {
    pub decision: AutoModeDecision,
    /// For `blocked`, the deny rule that matched. For `allowed`, the allow
    /// rule that overrode a matching deny rule, if one did.
    pub rule: Option<String>,
    /// How likely the decision model judged `rule` to apply, 0 to 1.
    pub probability: Option<f64>,
    /// For `unchecked`, why the check couldn't run, in plain words.
    pub reason: Option<String>,
    /// Input tokens the check cost.
    #[ts(type = "number | null")]
    pub input_tokens: Option<u64>,
}

impl AutoModeVerdict {
    fn unchecked(reason: String) -> Self {
        Self {
            decision: AutoModeDecision::Unchecked,
            rule: None,
            probability: None,
            reason: Some(reason),
            input_tokens: None,
        }
    }

    /// The tool result the agent sees in place of a blocked call's output.
    #[must_use]
    pub fn blocked_message(&self) -> String {
        let rule = self.rule.as_deref().unwrap_or("one of the deny rules");
        let sure = self
            .probability
            .map(|p| format!(" ({:.0}% sure)", p * 100.0))
            .unwrap_or_default();
        format!(
            "Auto Mode blocked this call, so it did not run: it matches the rule \"{rule}\"{sure}. \
             Try a different approach that the rule allows, or ask the user to do it or to change the rule."
        )
    }
}

/// One agent's Auto Mode: its rules and the shared decision model. Shared by
/// the agent and its sessions, so a config reload reaches both.
pub struct AutoModeGate {
    agent: String,
    config: RwLock<AutoModeConfig>,
    system_one: Arc<SystemOneService>,
}

/// A handle to an agent's [`AutoModeGate`].
pub type SharedAutoMode = Arc<AutoModeGate>;

impl AutoModeGate {
    #[must_use]
    pub fn new_shared(
        agent: impl Into<String>,
        config: AutoModeConfig,
        system_one: Arc<SystemOneService>,
    ) -> SharedAutoMode {
        Arc::new(Self {
            agent: agent.into(),
            config: RwLock::new(config),
            system_one,
        })
    }

    /// Take a reloaded `[auto_mode]`.
    pub fn set_config(&self, config: AutoModeConfig) {
        *self.config.write().unwrap_or_else(PoisonError::into_inner) = config;
    }

    fn config(&self) -> AutoModeConfig {
        self.config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Check `call` against the rules. `None` when Auto Mode is off or has
    /// no deny rules, so nothing was checked.
    pub async fn check(
        &self,
        call: &ToolCall,
        latest_user_request: Option<&str>,
    ) -> Option<AutoModeVerdict> {
        let config = self.config();
        if !config.is_active() {
            return None;
        }
        let state = call_state(&self.agent, call, latest_user_request);
        let questions = rule_questions(&config);
        let verdict = match self.system_one.evaluate(&state, &questions).await {
            Ok(response) => judge(&config, &response),
            Err(e) => AutoModeVerdict::unchecked(e.user_message()),
        };
        log_verdict(&call.name, &verdict);
        Some(verdict)
    }
}

fn log_verdict(tool_name: &str, verdict: &AutoModeVerdict) {
    match verdict.decision {
        AutoModeDecision::Blocked => tracing::info!(
            tool_name,
            rule = verdict.rule.as_deref().unwrap_or_default(),
            probability = verdict.probability,
            input_tokens = verdict.input_tokens,
            "auto mode blocked a tool call"
        ),
        AutoModeDecision::Allowed => tracing::debug!(
            tool_name,
            override_rule = verdict.rule.as_deref(),
            input_tokens = verdict.input_tokens,
            "auto mode allowed a tool call"
        ),
        // The service's own status carries the outage; one log per call
        // would repeat it for every tool call until it's back.
        AutoModeDecision::Unchecked => tracing::debug!(
            tool_name,
            reason = verdict.reason.as_deref(),
            "auto mode could not check a tool call"
        ),
    }
}

/// The text of the most recent user message, for judging rules about what
/// the user asked for.
#[must_use]
pub fn latest_user_request(messages: &[crate::inference::Message]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == crate::inference::Role::User && !m.content.trim().is_empty())
        .map(|m| m.content.as_str())
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}… [truncated]")
}

fn call_state(agent: &str, call: &ToolCall, latest_user_request: Option<&str>) -> Value {
    let arguments_text = call.arguments.to_string();
    let arguments = if arguments_text.chars().count() > MAX_ARGUMENT_CHARS {
        Value::String(truncate_chars(&arguments_text, MAX_ARGUMENT_CHARS))
    } else {
        call.arguments.clone()
    };
    let mut call_value = json!({
        "agent": agent,
        "tool": call.name,
        "arguments": arguments,
    });
    if let (Some(server), Some(obj)) = (&call.server, call_value.as_object_mut()) {
        obj.insert("tool_server".to_string(), Value::String(server.clone()));
    }
    json!({
        "call": call_value,
        "latest_user_request": latest_user_request
            .map(|r| truncate_chars(r, MAX_USER_REQUEST_CHARS)),
    })
}

fn rule_question(rule: &str) -> Question {
    Question::noul(json!({
        "rule": rule,
        "question": "Would carrying out the tool call in `call` do what `rule` describes?",
    }))
}

fn rule_questions(config: &AutoModeConfig) -> BTreeMap<String, Question> {
    let deny = config
        .deny
        .iter()
        .enumerate()
        .map(|(i, rule)| (format!("deny_{i}"), rule_question(rule)));
    let allow = config
        .allow
        .iter()
        .enumerate()
        .map(|(i, rule)| (format!("allow_{i}"), rule_question(rule)));
    deny.chain(allow).collect()
}

/// The highest-probability rule in `rules` at or above `threshold`, and the
/// highest probability seen; `Err` when an answer is missing.
fn strongest_match<'a>(
    rules: &'a [String],
    prefix: &str,
    response: &SystemOneResponse,
    threshold: f64,
) -> Result<Option<(&'a str, f64)>, String> {
    let mut best: Option<(&str, f64)> = None;
    for (i, rule) in rules.iter().enumerate() {
        let p = response
            .answers
            .get(&format!("{prefix}_{i}"))
            .and_then(crate::inference::system_one::Answer::noul)
            .ok_or_else(|| format!("The decision model didn't answer for the rule \"{rule}\"."))?;
        if p >= threshold && best.is_none_or(|(_, b)| p > b) {
            best = Some((rule, p));
        }
    }
    Ok(best)
}

fn judge(config: &AutoModeConfig, response: &SystemOneResponse) -> AutoModeVerdict {
    let input_tokens = Some(response.usage.input_tokens);
    let deny = strongest_match(&config.deny, "deny", response, config.threshold);
    let allow = strongest_match(&config.allow, "allow", response, config.threshold);
    let (deny, allow) = match (deny, allow) {
        (Ok(d), Ok(a)) => (d, a),
        (Err(reason), _) | (_, Err(reason)) => {
            return AutoModeVerdict {
                input_tokens,
                ..AutoModeVerdict::unchecked(reason)
            };
        }
    };
    match (deny, allow) {
        (None, _) => AutoModeVerdict {
            decision: AutoModeDecision::Allowed,
            rule: None,
            probability: None,
            reason: None,
            input_tokens,
        },
        (Some(_), Some((rule, p))) => AutoModeVerdict {
            decision: AutoModeDecision::Allowed,
            rule: Some(rule.to_string()),
            probability: Some(p),
            reason: None,
            input_tokens,
        },
        (Some((rule, p)), None) => AutoModeVerdict {
            decision: AutoModeDecision::Blocked,
            rule: Some(rule.to_string()),
            probability: Some(p),
            reason: None,
            input_tokens,
        },
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::{SystemOneConfig, SystemOneProvider};

    fn config(deny: &[&str], allow: &[&str]) -> AutoModeConfig {
        AutoModeConfig {
            enabled: true,
            deny: deny.iter().map(|s| (*s).to_string()).collect(),
            allow: allow.iter().map(|s| (*s).to_string()).collect(),
            threshold: 0.5,
        }
    }

    fn response(answers: &[(&str, f64)]) -> SystemOneResponse {
        let answers: serde_json::Map<String, Value> = answers
            .iter()
            .map(|(k, p)| ((*k).to_string(), json!({ "type": "noul", "noul": p })))
            .collect();
        serde_json::from_value(json!({
            "model": "jev-1.13.0",
            "answers": answers,
            "usage": { "input_tokens": 40, "output_tokens": 2 }
        }))
        .unwrap()
    }

    fn call() -> ToolCall {
        ToolCall {
            id: "c1".to_string(),
            name: "exec".to_string(),
            arguments: json!({ "command": "git push origin main" }),
            server: None,
        }
    }

    #[test]
    fn a_matching_deny_rule_blocks() {
        let verdict = judge(
            &config(&["Pushing to main", "Deleting files"], &[]),
            &response(&[("deny_0", 0.93), ("deny_1", 0.02)]),
        );
        assert_eq!(verdict.decision, AutoModeDecision::Blocked);
        assert_eq!(verdict.rule.as_deref(), Some("Pushing to main"));
        assert_eq!(verdict.input_tokens, Some(40));
        assert!(verdict.blocked_message().contains("Pushing to main"));
        assert!(verdict.blocked_message().contains("93%"));
    }

    #[test]
    fn a_matching_allow_rule_overrides_the_deny_rule() {
        let verdict = judge(
            &config(&["Deleting files"], &["Deleting files under tmp/"]),
            &response(&[("deny_0", 0.9), ("allow_0", 0.8)]),
        );
        assert_eq!(verdict.decision, AutoModeDecision::Allowed);
        assert_eq!(verdict.rule.as_deref(), Some("Deleting files under tmp/"));
    }

    #[test]
    fn below_threshold_is_allowed() {
        let verdict = judge(
            &config(&["Pushing to main"], &[]),
            &response(&[("deny_0", 0.49)]),
        );
        assert_eq!(verdict.decision, AutoModeDecision::Allowed);
        assert_eq!(verdict.rule, None);
    }

    #[test]
    fn a_missing_answer_is_unchecked() {
        let verdict = judge(&config(&["a", "b"], &[]), &response(&[("deny_0", 0.1)]));
        assert_eq!(verdict.decision, AutoModeDecision::Unchecked);
        assert!(verdict.reason.unwrap().contains("\"b\""));
    }

    #[test]
    fn state_carries_the_call_and_truncates_huge_arguments() {
        let mut big = call();
        big.arguments = json!({ "content": "x".repeat(MAX_ARGUMENT_CHARS * 2) });
        big.server = Some("github".to_string());
        let state = call_state("scout", &big, Some("ship it"));
        let args = state.pointer("/call/arguments").unwrap();
        assert!(args.as_str().unwrap().ends_with("[truncated]"));
        assert_eq!(state.pointer("/call/tool_server"), Some(&json!("github")));
        assert_eq!(
            state.pointer("/latest_user_request"),
            Some(&json!("ship it"))
        );
    }

    #[test]
    fn latest_user_request_skips_assistant_and_tool_messages() {
        let messages = vec![
            crate::inference::Message::user("first ask"),
            crate::inference::Message::user("deploy the site"),
            crate::inference::Message::assistant("on it", None),
            crate::inference::Message::tool("ok", "c1"),
        ];
        assert_eq!(latest_user_request(&messages), Some("deploy the site"));
    }

    #[tokio::test]
    async fn inactive_gate_checks_nothing() {
        let gate = AutoModeGate::new_shared(
            "scout",
            AutoModeConfig::default(),
            SystemOneService::new(None),
        );
        assert_eq!(gate.check(&call(), None).await, None);
    }

    #[tokio::test]
    async fn an_unconfigured_decision_model_leaves_the_call_unchecked() {
        let gate = AutoModeGate::new_shared(
            "scout",
            config(&["Pushing to main"], &[]),
            SystemOneService::new(None),
        );
        let verdict = gate.check(&call(), None).await.unwrap();
        assert_eq!(verdict.decision, AutoModeDecision::Unchecked);
        assert!(verdict.reason.unwrap().contains("No decision model"));
    }

    #[tokio::test]
    async fn the_gate_asks_every_rule_in_one_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nimble",
                "answers": {
                    "deny_0": { "type": "noul", "noul": 0.97 },
                    "allow_0": { "type": "noul", "noul": 0.03 }
                },
                "usage": { "input_tokens": 55, "output_tokens": 2 }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = SystemOneService::new(Some(&SystemOneConfig {
            provider: SystemOneProvider::Ollama,
            url: server.uri(),
            model: "nimble".to_string(),
            api_key: None,
            keep_alive: None,
        }));
        let gate = AutoModeGate::new_shared(
            "scout",
            config(&["Pushing to main"], &["Pushing to a feature branch"]),
            service,
        );
        let verdict = gate.check(&call(), Some("push it")).await.unwrap();
        assert_eq!(verdict.decision, AutoModeDecision::Blocked);
        assert_eq!(verdict.input_tokens, Some(55));

        gate.set_config(AutoModeConfig::default());
        assert_eq!(
            gate.check(&call(), None).await,
            None,
            "a reloaded config that turns Auto Mode off takes effect"
        );
    }
}
