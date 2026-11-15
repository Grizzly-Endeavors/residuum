//! `@mention` and `@everyone` parsing for owner messages and agent baton passes.

use crate::types::RoomSeat;

/// Parse an owner message for `@<seat>` mentions and `@everyone`.
///
/// Matches `@<slug>` (including full `instance/agent` or leaf `agent` for
/// sibling A2A seats) and `@<display_name>` case-insensitively on word
/// boundaries. When `@everyone` is present alongside explicit `@<seat>` mentions,
/// explicitly named seats appear first in mention order, followed by the
/// remaining active roster seats in roster order. Deduplicated.
#[must_use]
pub fn parse_owner_mentions(text: &str, roster: &[RoomSeat]) -> Vec<String> {
    let (mut explicit, saw_everyone) = scan_mentions(text, roster, None, true);
    if saw_everyone {
        for seat in roster {
            if !explicit.contains(&seat.seat_id) {
                explicit.push(seat.seat_id.clone());
            }
        }
    }
    explicit
}

/// Parse an agent's reply for explicit `@<seat>` baton-pass mentions.
///
/// Matches `@<slug>` and `@<display_name>` case-insensitively on word
/// boundaries, excluding `speaker_seat_id` (no self-queuing) and deduplicating
/// in mention order. `@everyone` is not expanded for agent replies.
#[must_use]
pub fn parse_agent_baton_mentions(
    text: &str,
    speaker_seat_id: &str,
    roster: &[RoomSeat],
) -> Vec<String> {
    let (mentions, _) = scan_mentions(text, roster, Some(speaker_seat_id), false);
    mentions
}

fn is_boundary_before(text: &str, at_byte: usize) -> bool {
    text.get(..at_byte)
        .and_then(|prefix| prefix.chars().next_back())
        .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_')
}

fn is_boundary_after(after: &str) -> bool {
    after
        .chars()
        .next()
        .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_' && ch != '-')
}

fn strip_ci_prefix<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    if needle.is_empty() {
        return None;
    }
    let mut h_chars = haystack.char_indices().peekable();
    for n_ch in needle.chars() {
        let (_, h_ch) = h_chars.next()?;
        if !n_ch.to_lowercase().eq(h_ch.to_lowercase()) {
            return None;
        }
    }
    let end = h_chars.peek().map_or(haystack.len(), |&(idx, _)| idx);
    haystack.get(end..)
}

fn scan_mentions(
    text: &str,
    roster: &[RoomSeat],
    exclude_seat_id: Option<&str>,
    allow_everyone: bool,
) -> (Vec<String>, bool) {
    let mut explicit: Vec<String> = Vec::new();
    let mut saw_everyone = false;
    let mut skip_until = 0_usize;

    for (at_idx, ch) in text.char_indices() {
        if at_idx < skip_until || ch != '@' || !is_boundary_before(text, at_idx) {
            continue;
        }
        let Some(after_at) = text.get(at_idx + 1..) else {
            continue;
        };

        if allow_everyone
            && let Some(rest) = strip_ci_prefix(after_at, "everyone")
            && is_boundary_after(rest)
        {
            saw_everyone = true;
            skip_until = at_idx + 1 + (after_at.len() - rest.len());
            continue;
        }

        let mut best: Option<(usize, &str)> = None;
        for seat in roster {
            let candidates = [
                seat.display_name.as_str(),
                seat.leaf_slug(),
                seat.full_slug(),
            ];
            for candidate in candidates {
                let Some(rest) = strip_ci_prefix(after_at, candidate) else {
                    continue;
                };
                if !is_boundary_after(rest) {
                    continue;
                }
                let matched_len = after_at.len() - rest.len();
                let should_replace = match best {
                    None => true,
                    Some((best_len, best_id)) => {
                        matched_len > best_len
                            || (matched_len == best_len
                                && exclude_seat_id == Some(best_id)
                                && exclude_seat_id != Some(seat.seat_id.as_str()))
                    }
                };
                if should_replace {
                    best = Some((matched_len, seat.seat_id.as_str()));
                }
            }
        }

        if let Some((matched_len, seat_id)) = best {
            skip_until = at_idx + 1 + matched_len;
            if exclude_seat_id != Some(seat_id) && !explicit.iter().any(|s| s == seat_id) {
                explicit.push(seat_id.to_owned());
            }
        }
    }

    (explicit, saw_everyone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RoomSeatKind;

    fn sample_roster() -> Vec<RoomSeat> {
        vec![
            RoomSeat {
                seat_id: "agent:scout".to_owned(),
                kind: RoomSeatKind::Local,
                display_name: "Scout".to_owned(),
                role: Some("Research".to_owned()),
                instance: None,
            },
            RoomSeat {
                seat_id: "agent:analyst".to_owned(),
                kind: RoomSeatKind::Local,
                display_name: "Analyst".to_owned(),
                role: Some("Data".to_owned()),
                instance: None,
            },
            RoomSeat {
                seat_id: "agent:builder".to_owned(),
                kind: RoomSeatKind::Local,
                display_name: "Builder".to_owned(),
                role: Some("Engineering".to_owned()),
                instance: None,
            },
            RoomSeat {
                seat_id: "a2a:ops/planner".to_owned(),
                kind: RoomSeatKind::SiblingA2a,
                display_name: "Ops Planner".to_owned(),
                role: Some("Operations".to_owned()),
                instance: Some("ops".to_owned()),
            },
            RoomSeat {
                seat_id: "a2a:partner".to_owned(),
                kind: RoomSeatKind::ExternalA2a,
                display_name: "Partner Bot".to_owned(),
                role: Some("External Advisor".to_owned()),
                instance: None,
            },
        ]
    }

    #[test]
    fn owner_mentions_plain_text_returns_empty() {
        let roster = sample_roster();
        assert!(
            parse_owner_mentions("Quick check on priorities today?", &roster).is_empty(),
            "plain text without @mentions should return empty"
        );
        assert!(
            parse_owner_mentions("Email user@scout.example.com please", &roster).is_empty(),
            "email addresses must not trigger @mentions"
        );
        assert!(
            parse_owner_mentions("Checking @scouting progress", &roster).is_empty(),
            "partial prefix match without boundary must not trigger @mention"
        );
    }

    #[test]
    fn owner_mentions_deduplicates_in_mention_order() {
        let roster = sample_roster();
        assert_eq!(
            parse_owner_mentions("@scout @analyst @scout", &roster),
            vec!["agent:scout", "agent:analyst"],
            "duplicate mentions should keep first-seen mention order"
        );
        assert_eq!(
            parse_owner_mentions("Hey @Analyst, ask @Scout!", &roster),
            vec!["agent:analyst", "agent:scout"],
            "case-insensitive display_name mentions with punctuation should resolve"
        );
    }

    #[test]
    fn owner_mentions_everyone_and_mixed_ordering() {
        let roster = sample_roster();
        assert_eq!(
            parse_owner_mentions("@everyone quick sync", &roster),
            vec![
                "agent:scout",
                "agent:analyst",
                "agent:builder",
                "a2a:ops/planner",
                "a2a:partner",
            ],
            "@everyone alone should return full roster in roster order"
        );
        assert_eq!(
            parse_owner_mentions("@everyone — @builder start", &roster),
            vec![
                "agent:builder",
                "agent:scout",
                "agent:analyst",
                "a2a:ops/planner",
                "a2a:partner",
            ],
            "mixed @everyone + @builder should put builder first then remaining roster"
        );
    }

    #[test]
    fn agent_baton_mentions_excludes_self_and_matches_external_and_sibling() {
        let roster = sample_roster();
        assert_eq!(
            parse_agent_baton_mentions(
                "@Scout handoff to @Ops Planner and @partner, cc @planner",
                "agent:scout",
                &roster
            ),
            vec!["a2a:ops/planner", "a2a:partner"],
            "self-mention must be excluded and sibling/external seats matched by display_name and slug once"
        );
        assert_eq!(
            parse_agent_baton_mentions(
                "@Partner Bot please check @ops/planner",
                "agent:builder",
                &roster
            ),
            vec!["a2a:partner", "a2a:ops/planner"],
            "multi-word display_name and qualified sibling slug should both match"
        );
        assert!(
            parse_agent_baton_mentions("@everyone please chime in", "agent:scout", &roster)
                .is_empty(),
            "@everyone in an agent reply must not expand"
        );
    }
}
