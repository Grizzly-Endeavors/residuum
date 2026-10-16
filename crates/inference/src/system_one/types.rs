//! Wire types for the System 1 `/v1/systemone` evaluation API.
//!
//! `TypeSafe` defined the format and Ollama serves the same one, so one set of
//! types covers every System 1 provider.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A typed question asked about a request's `state`.
///
/// `instructions` and criteria are JSON values rather than strings because
/// the API accepts structured instructions: an object holding the question in
/// one field and the data it refers to in others.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    /// A yes/no question; the answer is the probability of yes.
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// Pick one option from a defined set.
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Option<Value>>,
    },
    /// Rate along ordered levels, lowest first.
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

impl Question {
    /// A yes/no question with no criteria.
    #[must_use]
    pub fn noul(instructions: impl Into<Value>) -> Self {
        Self::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }
}

/// What a yes and a no mean for a [`Question::Noul`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: Value,
    #[serde(rename = "false")]
    pub no: Value,
}

/// The request body of `POST /v1/systemone`.
#[derive(Debug, Serialize)]
pub(crate) struct SystemOneRequest<'a> {
    pub model: &'a str,
    pub state: &'a Value,
    pub questions: &'a BTreeMap<String, Question>,
    /// Ollama-only: how long the model stays loaded after the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<&'a str>,
}

/// One answer, typed to match its question.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Noul {
        /// Probability of yes, 0 to 1.
        noul: f64,
    },
    Choice {
        choice: String,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        confidence: Option<f64>,
    },
    Score {
        score: f64,
        #[serde(default)]
        legend: BTreeMap<String, String>,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        confidence: Option<f64>,
    },
}

impl Answer {
    /// The yes probability of a noul answer; `None` for other answer types.
    #[must_use]
    pub fn noul(&self) -> Option<f64> {
        match self {
            Self::Noul { noul } => Some(*noul),
            Self::Choice { .. } | Self::Score { .. } => None,
        }
    }
}

/// Token usage reported for one evaluation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct SystemOneUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// The response body of `POST /v1/systemone`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SystemOneResponse {
    /// The model that answered; a versioned ID even when an alias was sent.
    #[serde(default)]
    pub model: String,
    /// One answer per question, under the question's own key.
    pub answers: BTreeMap<String, Answer>,
    #[serde(default)]
    pub usage: SystemOneUsage,
}

/// A model name the provider accepts in the `model` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemOneModel {
    pub name: String,
    pub description: Option<String>,
}

/// `GET /v1/models` in either shape a System 1 provider returns: `TypeSafe`'s
/// `{ models: [{ name, description }] }` or Ollama's OpenAI-style
/// `{ data: [{ id }] }`.
#[derive(Debug, Deserialize)]
pub(crate) struct ModelListResponse {
    #[serde(default)]
    models: Vec<TypeSafeModelCard>,
    #[serde(default)]
    data: Vec<OpenAiModelCard>,
}

#[derive(Debug, Deserialize)]
struct TypeSafeModelCard {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelCard {
    id: String,
}

impl ModelListResponse {
    pub(crate) fn into_models(self) -> Vec<SystemOneModel> {
        let typesafe = self.models.into_iter().map(|m| SystemOneModel {
            name: m.name,
            description: m.description,
        });
        let openai = self.data.into_iter().map(|m| SystemOneModel {
            name: m.id,
            description: None,
        });
        typesafe.chain(openai).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn noul_question_serializes_without_criteria() {
        let q = Question::noul("Is this urgent?");
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({ "type": "noul", "instructions": "Is this urgent?" }),
            "a bare noul should carry only type and instructions"
        );
    }

    #[test]
    fn noul_criteria_use_true_false_keys() {
        let q = Question::Noul {
            instructions: json!("Is this urgent?"),
            criteria: Some(NoulCriteria {
                yes: json!("time-sensitive"),
                no: json!("not urgent"),
            }),
        };
        assert_eq!(
            serde_json::to_value(&q).unwrap().get("criteria"),
            Some(&json!({ "true": "time-sensitive", "false": "not urgent" })),
            "criteria should use the API's true/false keys"
        );
    }

    #[test]
    fn score_and_choice_answers_parse() {
        let body = json!({
            "model": "jev-1.13.0",
            "answers": {
                "team": {
                    "type": "choice",
                    "choice": "billing",
                    "probabilities": { "billing": 0.9, "tech": 0.1 },
                    "confidence": 0.8
                },
                "anger": {
                    "type": "score",
                    "score": 1.05,
                    "legend": { "0": "Calm", "1": "Angry" },
                    "probabilities": { "0": 0.0, "1": 1.0 },
                    "confidence": 0.9
                }
            },
            "usage": { "input_tokens": 10, "output_tokens": 2 }
        });
        let parsed: SystemOneResponse = serde_json::from_value(body).unwrap();
        assert!(
            matches!(parsed.answers.get("team"), Some(Answer::Choice { choice, .. }) if choice == "billing"),
            "choice answer should parse"
        );
        assert!(
            matches!(parsed.answers.get("anger"), Some(Answer::Score { score, .. }) if (score - 1.05).abs() < 1e-9),
            "score answer should parse"
        );
        assert_eq!(parsed.usage.input_tokens, 10, "usage should parse");
    }

    #[test]
    fn model_list_parses_both_shapes() {
        let typesafe: ModelListResponse = serde_json::from_value(json!({
            "models": [{ "name": "jev-latest", "description": "flagship" }]
        }))
        .unwrap();
        let ollama: ModelListResponse = serde_json::from_value(json!({
            "object": "list",
            "data": [{ "id": "nimble", "object": "model" }]
        }))
        .unwrap();
        let first = |list: ModelListResponse| list.into_models().first().map(|m| m.name.clone());
        assert_eq!(
            first(typesafe).as_deref(),
            Some("jev-latest"),
            "typesafe shape should parse"
        );
        assert_eq!(
            first(ollama).as_deref(),
            Some("nimble"),
            "openai shape should parse"
        );
    }
}
