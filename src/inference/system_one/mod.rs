//! System 1 (decision model) inference: typed judgments from `TypeSafe`'s Jev,
//! Ollama's decision models, or any endpoint serving the same
//! `/v1/systemone` API.

mod client;
mod error;
mod types;

pub use client::{SystemOneClient, SystemOneEndpoint};
pub use error::{SystemOneError, SystemOneOutageKind};
pub use types::{
    Answer, NoulCriteria, Question, SystemOneModel, SystemOneResponse, SystemOneUsage,
};
