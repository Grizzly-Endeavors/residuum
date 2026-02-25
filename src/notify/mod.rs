//! Notification routing: NOTIFY.yml-based dispatch to built-in and external channels.
//!
//! Background task results (pulse checks, cron jobs) are routed through
//! `NotificationRouter` which reads `NOTIFY.yml` to determine which channels
//! should receive each task's results.
//!
//! Built-in channels:
//! - `agent_wake` — inject into agent feed, start a turn if idle
//! - `agent_feed` — inject into agent feed, wait for next interaction
//! - `inbox` — store as an `InboxItem`, surface as unread count
//!
//! External channels (ntfy, webhook) are configured in `config.toml` under
//! `[notifications.channels]`.

pub mod channels;
pub mod external;
pub mod loader;
pub mod router;
pub mod types;
