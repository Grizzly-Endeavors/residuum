mod chunk_extractor;
#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;
pub mod episode_store;
mod fork_safe_directory;
pub mod log_store;
pub mod merge_writer;
mod merged_run_log;
pub mod observer;
pub mod recent_context;
pub mod recent_messages;
pub mod reflector;
pub mod search;
pub mod team_wiki;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod tokens;
pub mod types;
pub mod vector_store;
pub mod wiki_index;

pub use chunk_extractor::read_idx_jsonl;

pub(crate) fn truncate_at_char_boundary(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    let prefix = s.split_at(end).0;
    format!("{prefix}...(truncated)")
}

pub(crate) fn parse_minute_timestamp(ts: &str, tz: chrono_tz::Tz) -> chrono::NaiveDateTime {
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M") {
        return naive;
    }
    tracing::warn!(
        timestamp = ts,
        "failed to parse timestamp, using current time"
    );
    residuum_core::time::now_local(tz)
}
