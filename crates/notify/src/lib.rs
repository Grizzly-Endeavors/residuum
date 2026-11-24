pub mod batch_aggregator;
mod channel_builder;
pub mod channels;
pub mod external;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod router;
pub mod subscriber;
#[cfg(target_os = "windows")]
pub mod windows;

pub use channel_builder::build_external_channels;

#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;
