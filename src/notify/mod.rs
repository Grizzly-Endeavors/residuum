//! Notification channels: deliver events to external services and the inbox.
//!
//! Each notification channel subscribes to its bus topic and delivers events
//! independently. Channels are configured in `channels.toml`.

pub mod channels;
pub mod external;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod router;
pub mod subscriber;
pub mod types;
#[cfg(target_os = "windows")]
pub mod windows;
