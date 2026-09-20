//! Vendor adapters implementing [`InferenceProvider`](crate::inference::InferenceProvider).
//!
//! Each module speaks one vendor's wire protocol and translates it to and from
//! the shared inference vocabulary. Composition of providers (retry, failover,
//! chain construction) lives in the parent module, not here.

pub(crate) mod anthropic;
pub(crate) mod gemini;
pub(crate) mod null;
pub(crate) mod ollama;
pub(crate) mod openai;
