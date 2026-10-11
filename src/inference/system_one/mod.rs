//! System 1 (decision model) inference: typed judgments from `TypeSafe`'s Jev,
//! Ollama's decision models, or any endpoint serving the same
//! `/v1/systemone` API.

mod client;
mod error;
mod health;
mod service;
mod types;

pub use client::{SystemOneClient, SystemOneEndpoint};
pub use error::{SystemOneError, SystemOneOutageKind};
pub use health::{SystemOneOutage, SystemOneStatus};
pub use service::{SystemOneService, client_for_config, endpoint_from_config};
pub use types::{
    Answer, NoulCriteria, Question, SystemOneModel, SystemOneResponse, SystemOneUsage,
};
