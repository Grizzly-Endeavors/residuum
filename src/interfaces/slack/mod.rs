//! Slack interface — Socket Mode, DM-only MVP (design: docs/design/slack-interface.md).
//!
//! One interface instance per agent, each with its own app tokens and socket,
//! mirroring the Telegram adapter's shape. Inbound DM events deliver into the
//! agent's main conversation; outbound replies ride the conversation-only
//! contract shared by every external chat interface.
pub(crate) mod api;
pub(crate) mod events;
