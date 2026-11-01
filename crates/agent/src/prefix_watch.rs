//! Spots model requests whose earlier parts differ from the conversation's
//! previous request.
//!
//! Providers cache a request by its exact byte prefix: Anthropic renders the
//! tools, then the system prompt, then the messages, and the others reuse the
//! longest prefix that matches. A request that changes a part the previous
//! request already sent loses the cache from that part on. This module
//! compares each request a conversation sends with the one before it, part
//! by part (tools, system prompt, each earlier message), and logs every
//! change with the part and the first index that differs, so a lost cache
//! shows up in the logs.
//!
//! It keeps fingerprints, not copies: one hash per tool and per message, and
//! the system prompt's text (to name the byte and section that changed).

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hasher};

use residuum_inference::{Message, Role, ToolDefinition};
use serde::Serialize;

use crate::turn::ModelRequest;

/// Why an earlier part of a request differs from the previous request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Cause {
    /// A tool was added or removed: a config reload or an MCP server
    /// connecting or leaving.
    ToolSetChanged,
    /// The same tools went out in a different order.
    ToolOrderChanged,
    /// Between turns, a tool's definition changed while the set and order
    /// stayed: a reload rewrote a description or schema, or a definition does
    /// not render the same way twice.
    ToolDefinitionChanged,
    /// Within one turn, a tool's definition changed while the set and order
    /// stayed. Nothing reloads tools mid-turn.
    ToolDefinitionChangedMidTurn,
    /// Between turns, the system prompt changed: an identity file, the team,
    /// memory or the skills changed, or a section does not render the same
    /// way twice.
    SystemChanged,
    /// The system prompt changed in the same request as a history rewrite:
    /// the observer moved the conversation into memory.
    SystemChangedWithHistory,
    /// Within one turn, the system prompt changed. Its inputs are fixed for
    /// the turn.
    SystemChangedMidTurn,
    /// The agent rewrote its history (observer rotation or a clear).
    HistoryRewritten,
    /// A message the previous request sent changed or went missing, and the
    /// history was not rewritten.
    HistoryChanged,
}

/// How loudly a change is logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    /// An expected change.
    Debug,
    /// A change with a legitimate source that the code can't tell apart
    /// from a rendering bug.
    Info,
    /// A change nothing legitimate makes.
    Warn,
}

impl Cause {
    /// The request part this cause changes.
    fn part(self) -> &'static str {
        match self {
            Self::ToolSetChanged
            | Self::ToolOrderChanged
            | Self::ToolDefinitionChanged
            | Self::ToolDefinitionChangedMidTurn => "tools",
            Self::SystemChanged | Self::SystemChangedWithHistory | Self::SystemChangedMidTurn => {
                "system"
            }
            Self::HistoryRewritten | Self::HistoryChanged => "messages",
        }
    }

    /// The level the first change with this cause logs at.
    fn level(self) -> Level {
        match self {
            Self::ToolSetChanged | Self::SystemChangedWithHistory | Self::HistoryRewritten => {
                Level::Debug
            }
            Self::ToolDefinitionChanged | Self::SystemChanged => Level::Info,
            Self::ToolOrderChanged
            | Self::ToolDefinitionChangedMidTurn
            | Self::SystemChangedMidTurn
            | Self::HistoryChanged => Level::Warn,
        }
    }

    /// The cause as a log field.
    fn name(self) -> &'static str {
        match self {
            Self::ToolSetChanged => "tool_set_changed",
            Self::ToolOrderChanged => "tool_order_changed",
            Self::ToolDefinitionChanged => "tool_definition_changed",
            Self::ToolDefinitionChangedMidTurn => "tool_definition_changed_mid_turn",
            Self::SystemChanged => "system_changed",
            Self::SystemChangedWithHistory => "system_changed_with_history",
            Self::SystemChangedMidTurn => "system_changed_mid_turn",
            Self::HistoryRewritten => "history_rewritten",
            Self::HistoryChanged => "history_changed",
        }
    }
}

/// One earlier part of a request that differs from the previous request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Divergence {
    pub(crate) cause: Cause,
    /// The first index that differs: a position in the tool list, a byte
    /// offset in the system prompt, or a position in the request's messages
    /// (the system prompt is message 0).
    pub(crate) index: usize,
    /// For a system prompt change, the section the first changed byte is in.
    pub(crate) section: Option<String>,
    /// The level this change logs at.
    pub(crate) level: Level,
}

/// What a request sent, as fingerprints.
struct SentParts {
    /// Each tool's name and a hash of its whole definition, in request order.
    tools: Vec<(String, u64)>,
    /// The system prompt's text.
    system: String,
    /// A hash of each message after the system prompt.
    history: Vec<u64>,
}

/// Watches the requests one conversation sends.
///
/// The first change with a given cause logs at that cause's level; later
/// changes with the same cause log at `debug`, so a recurring break shows up
/// once at `warn` or `info` without flooding the log.
#[derive(Default)]
pub(crate) struct PrefixWatch {
    last: Option<SentParts>,
    rewrite_pending: bool,
    reported: BTreeSet<Cause>,
}

impl PrefixWatch {
    /// Note that the history was rewritten (rotated or cleared), so the next
    /// request's history change is expected.
    pub(crate) fn history_rewritten(&mut self) {
        self.rewrite_pending = true;
    }

    /// Compare `request` with the previous request and log each earlier
    /// part that changed. `within_turn` is whether `request` is a later call
    /// of the same turn as the previous one.
    pub(crate) fn observe(&mut self, request: &ModelRequest, within_turn: bool) {
        let conversation = request
            .options
            .cache
            .conversation_key
            .as_deref()
            .unwrap_or("unkeyed");
        for divergence in self.compare(request, within_turn) {
            log_divergence(conversation, &divergence);
        }
    }

    /// The earlier parts of `request` that differ from the previous request,
    /// recording `request` as the one the next request is compared with.
    pub(crate) fn compare(&mut self, request: &ModelRequest, within_turn: bool) -> Vec<Divergence> {
        let rewritten = std::mem::take(&mut self.rewrite_pending);
        let sent = match SentParts::of(request) {
            Ok(sent) => sent,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "couldn't fingerprint a model request; the next request's prompt-cache prefix goes unchecked"
                );
                self.last = None;
                return Vec::new();
            }
        };
        let found = self
            .last
            .as_ref()
            .map(|last| diff(last, &sent, within_turn, rewritten))
            .unwrap_or_default();
        self.last = Some(sent);

        found
            .into_iter()
            .map(|(cause, index, section)| Divergence {
                cause,
                index,
                section,
                level: self.level_for(cause),
            })
            .collect()
    }

    /// The level a change with `cause` logs at now: its own the first time,
    /// `debug` after that.
    fn level_for(&mut self, cause: Cause) -> Level {
        let level = cause.level();
        if level == Level::Debug || !self.reported.insert(cause) {
            Level::Debug
        } else {
            level
        }
    }
}

impl SentParts {
    fn of(request: &ModelRequest) -> Result<Self, serde_json::Error> {
        let tools = request
            .tools
            .iter()
            .map(|tool| Ok((tool.name.clone(), fingerprint(tool)?)))
            .collect::<Result<Vec<_>, serde_json::Error>>()?;
        let (system, history) = match request.messages.split_first() {
            Some((first, rest)) if first.role == Role::System => (first.content.clone(), rest),
            _ => (String::new(), request.messages.as_slice()),
        };
        let history = history
            .iter()
            .map(fingerprint::<Message>)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            tools,
            system,
            history,
        })
    }
}

/// Each earlier part of `sent` that differs from `last`, with its cause, the
/// first index that differs and, for the system prompt, the section.
fn diff(
    last: &SentParts,
    sent: &SentParts,
    within_turn: bool,
    rewritten: bool,
) -> Vec<(Cause, usize, Option<String>)> {
    let mut found = Vec::new();
    if let Some((cause, index)) = tools_change(&last.tools, &sent.tools, within_turn) {
        found.push((cause, index, None));
    }
    if last.system != sent.system {
        let index = common_prefix_len(&last.system, &sent.system);
        let cause = if within_turn {
            Cause::SystemChangedMidTurn
        } else if rewritten {
            Cause::SystemChangedWithHistory
        } else {
            Cause::SystemChanged
        };
        found.push((cause, index, section_at(&sent.system, index)));
    }
    if let Some(position) = first_missing(&last.history, &sent.history) {
        let cause = if rewritten {
            Cause::HistoryRewritten
        } else {
            Cause::HistoryChanged
        };
        // The system prompt is message 0.
        found.push((cause, position + 1, None));
    }
    found
}

/// How the tool list changed, with the first position that differs.
fn tools_change(
    last: &[(String, u64)],
    sent: &[(String, u64)],
    within_turn: bool,
) -> Option<(Cause, usize)> {
    let last_names: Vec<&str> = last.iter().map(|(name, _)| name.as_str()).collect();
    let sent_names: Vec<&str> = sent.iter().map(|(name, _)| name.as_str()).collect();
    if last_names != sent_names {
        let index = first_difference(&last_names, &sent_names);
        let last_set: BTreeSet<&str> = last_names.into_iter().collect();
        let sent_set: BTreeSet<&str> = sent_names.into_iter().collect();
        let cause = if last_set == sent_set {
            Cause::ToolOrderChanged
        } else {
            Cause::ToolSetChanged
        };
        return Some((cause, index));
    }
    if last == sent {
        return None;
    }
    let cause = if within_turn {
        Cause::ToolDefinitionChangedMidTurn
    } else {
        Cause::ToolDefinitionChanged
    };
    Some((cause, first_difference(last, sent)))
}

/// The first position where `a` and `b` differ (the shorter length when one
/// extends the other).
fn first_difference<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| a.len().min(b.len()))
}

/// The first position of `last` that `sent` does not repeat, when `sent`
/// does not start with all of `last`.
fn first_missing(last: &[u64], sent: &[u64]) -> Option<usize> {
    let index = first_difference(last, sent);
    (index < last.len()).then_some(index)
}

/// The length of the longest common byte prefix of `a` and `b`, rounded
/// down to a character boundary of `b`.
fn common_prefix_len(a: &str, b: &str) -> usize {
    let mut len = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while !b.is_char_boundary(len) {
        len -= 1;
    }
    len
}

/// The tag of the system prompt section the byte at `offset` is in. A
/// section is `<TAG>` at the start of the prompt or after a blank line; a
/// change that starts at the blank line before a section counts as that
/// section's.
fn section_at(system: &str, offset: usize) -> Option<String> {
    let mut end = offset.saturating_add(3).min(system.len());
    while !system.is_char_boundary(end) {
        end -= 1;
    }
    let head = system.get(..end)?;
    let start = head.rfind("\n\n<").map_or(0, |at| at + 2);
    let tag = system.get(start..)?.strip_prefix('<')?;
    let tag = tag.get(..tag.find('>')?)?;
    let is_tag = !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    is_tag.then(|| tag.to_string())
}

/// A hash of `value`'s JSON, streamed into the hasher.
fn fingerprint<T: Serialize>(value: &T) -> Result<u64, serde_json::Error> {
    let mut writer = HashWriter(DefaultHasher::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.0.finish())
}

/// Feeds written bytes to a hasher.
struct HashWriter(DefaultHasher);

impl std::io::Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Hasher::write(&mut self.0, buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The log line every change writes.
const DIVERGENCE_MESSAGE: &str = "prompt cache prefix changed: an earlier part of this model request differs from the conversation's previous request";

fn log_divergence(conversation: &str, divergence: &Divergence) {
    let cause = divergence.cause.name();
    let part = divergence.cause.part();
    let index = divergence.index;
    let section = divergence.section.as_deref().unwrap_or("");
    match divergence.level {
        Level::Debug => {
            tracing::debug!(
                conversation,
                part,
                index,
                section,
                cause,
                "{DIVERGENCE_MESSAGE}"
            );
        }
        Level::Info => {
            tracing::info!(
                conversation,
                part,
                index,
                section,
                cause,
                "{DIVERGENCE_MESSAGE}"
            );
        }
        Level::Warn => {
            tracing::warn!(
                conversation,
                part,
                index,
                section,
                cause,
                "{DIVERGENCE_MESSAGE}"
            );
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes known-length vectors for clarity"
)]
mod tests {
    use super::*;
    use residuum_inference::CompletionOptions;

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    const SYSTEM: &str =
        "<SOUL.md>\nme\n</SOUL.md>\n\n<HARNESS>\nh\n</HARNESS>\n\n<TEAM>\nalone\n</TEAM>";

    fn request(system: &str, history: &[&str], tools: &[ToolDefinition]) -> ModelRequest {
        let mut messages = vec![Message::system(system)];
        messages.extend(history.iter().map(|text| Message::user(*text)));
        ModelRequest {
            messages,
            tools: tools.to_vec(),
            options: CompletionOptions::default(),
        }
    }

    fn causes(found: &[Divergence]) -> Vec<(Cause, usize, Level)> {
        found.iter().map(|d| (d.cause, d.index, d.level)).collect()
    }

    fn two_tools() -> Vec<ToolDefinition> {
        vec![tool("read", "Read a file"), tool("exec", "Run a command")]
    }

    #[test]
    fn a_request_that_extends_the_previous_one_reports_nothing() {
        let mut watch = PrefixWatch::default();
        assert!(
            watch
                .compare(&request(SYSTEM, &["a"], &two_tools()), false)
                .is_empty()
        );
        let extended = watch.compare(&request(SYSTEM, &["a", "b", "c"], &two_tools()), true);
        assert!(
            extended.is_empty(),
            "appending is the expected shape: {extended:?}"
        );
    }

    #[test]
    fn a_changed_earlier_message_warns_once_with_its_position() {
        let mut watch = PrefixWatch::default();
        watch.compare(&request(SYSTEM, &["a", "b"], &two_tools()), false);

        let found = watch.compare(&request(SYSTEM, &["a", "B", "c"], &two_tools()), false);
        assert_eq!(
            causes(&found),
            vec![(Cause::HistoryChanged, 2, Level::Warn)],
            "message 2 (after the system prompt and 'a') changed"
        );

        let again = watch.compare(&request(SYSTEM, &["x"], &two_tools()), false);
        assert_eq!(
            causes(&again),
            vec![(Cause::HistoryChanged, 1, Level::Debug)],
            "the same cause logs at warn once per conversation"
        );
    }

    #[test]
    fn a_dropped_message_counts_as_a_change() {
        let mut watch = PrefixWatch::default();
        watch.compare(&request(SYSTEM, &["a", "b"], &two_tools()), false);
        let found = watch.compare(&request(SYSTEM, &["a"], &two_tools()), false);
        assert_eq!(
            causes(&found),
            vec![(Cause::HistoryChanged, 2, Level::Warn)]
        );
    }

    #[test]
    fn a_rewritten_history_is_expected_along_with_its_system_change() {
        let mut watch = PrefixWatch::default();
        watch.compare(&request(SYSTEM, &["a", "b", "c"], &two_tools()), false);
        watch.history_rewritten();

        let rotated = format!("{SYSTEM}\n\n<OBSERVATION_LOG>\na happened\n</OBSERVATION_LOG>");
        let found = watch.compare(&request(&rotated, &["c", "d"], &two_tools()), false);
        assert_eq!(
            causes(&found),
            vec![
                (Cause::SystemChangedWithHistory, SYSTEM.len(), Level::Debug),
                (Cause::HistoryRewritten, 1, Level::Debug),
            ]
        );
        assert_eq!(
            found.first().and_then(|d| d.section.as_deref()),
            Some("OBSERVATION_LOG"),
            "an appended section is named, not the one before it"
        );

        let next = watch.compare(&request(&rotated, &["c"], &two_tools()), false);
        assert_eq!(
            causes(&next),
            vec![(Cause::HistoryChanged, 2, Level::Warn)],
            "a rewrite covers only the request right after it"
        );
    }

    #[test]
    fn a_system_change_between_turns_is_info_and_within_a_turn_warns() {
        let edited = SYSTEM.replace("alone", "with a friend");

        let mut between = PrefixWatch::default();
        between.compare(&request(SYSTEM, &["a"], &two_tools()), false);
        let between_turns = between.compare(&request(&edited, &["a"], &two_tools()), false);
        assert_eq!(
            causes(&between_turns),
            vec![(
                Cause::SystemChanged,
                SYSTEM.find("alone").unwrap(),
                Level::Info
            )]
        );
        assert_eq!(between_turns[0].section.as_deref(), Some("TEAM"));

        let mut within = PrefixWatch::default();
        within.compare(&request(SYSTEM, &["a"], &two_tools()), false);
        let mid_turn = within.compare(&request(&edited, &["a"], &two_tools()), true);
        assert_eq!(mid_turn[0].cause, Cause::SystemChangedMidTurn);
        assert_eq!(mid_turn[0].level, Level::Warn);
    }

    #[test]
    fn tool_changes_are_told_apart_by_set_order_and_definition() {
        let mut watch = PrefixWatch::default();
        watch.compare(&request(SYSTEM, &[], &two_tools()), false);

        let mut added = two_tools();
        added.push(tool("mcp_search", "Search"));
        let joined = watch.compare(&request(SYSTEM, &[], &added), true);
        assert_eq!(
            causes(&joined),
            vec![(Cause::ToolSetChanged, 2, Level::Debug)],
            "a tool joining the end is a real change"
        );

        let mut reordered = added.clone();
        reordered.swap(0, 1);
        let moved = watch.compare(&request(SYSTEM, &[], &reordered), false);
        assert_eq!(
            causes(&moved),
            vec![(Cause::ToolOrderChanged, 0, Level::Warn)]
        );

        let mut redescribed = reordered.clone();
        redescribed[1].description = "Read a file, now in pages".to_string();
        let rewritten = watch.compare(&request(SYSTEM, &[], &redescribed), false);
        assert_eq!(
            causes(&rewritten),
            vec![(Cause::ToolDefinitionChanged, 1, Level::Info)]
        );

        let mut schema_changed = redescribed.clone();
        schema_changed[2].parameters = serde_json::json!({"type": "object", "required": []});
        let mid_turn = watch.compare(&request(SYSTEM, &[], &schema_changed), true);
        assert_eq!(
            causes(&mid_turn),
            vec![(Cause::ToolDefinitionChangedMidTurn, 2, Level::Warn)]
        );
    }

    #[test]
    fn the_changed_byte_is_on_a_character_boundary() {
        assert_eq!(common_prefix_len("aé", "aè"), 1);
        assert_eq!(common_prefix_len("same", "same"), 4);
        assert_eq!(common_prefix_len("same", "sameplus"), 4);
    }

    #[test]
    fn a_change_at_the_start_names_the_first_section() {
        assert_eq!(section_at(SYSTEM, 0).as_deref(), Some("SOUL.md"));
        assert_eq!(section_at("no sections here", 3), None);
    }
}
