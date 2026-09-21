//! Telegram group chats: deciding which messages are meant for the bot.
//!
//! In groups the bot acts only when addressed: an `@botname` mention, a reply
//! to one of its messages, or a command that is not aimed at another bot.
//! With Telegram's default privacy mode these are also the only group
//! messages a bot receives.

/// `group "Launch prep"`, or plain `group` when the chat has no title.
pub(super) fn group_label(title: Option<&str>) -> String {
    match title {
        Some(title) if !title.is_empty() => format!("group \"{title}\""),
        _ => "group".to_string(),
    }
}

/// Byte offsets of every case-insensitive `@username` in `text`, only where
/// the name is not the prefix of a longer one (`@bot` must not match `@bot2`).
fn mention_ranges(text: &str, username: &str) -> Vec<std::ops::Range<usize>> {
    let needle = format!("@{}", username.to_ascii_lowercase());
    // ASCII lowercasing keeps byte offsets aligned with `text`.
    let haystack = text.to_ascii_lowercase();
    haystack
        .match_indices(&needle)
        .map(|(start, m)| start..start + m.len())
        .filter(|range| {
            !haystack
                .get(range.end..)
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .collect()
}

fn without_ranges(text: &str, ranges: &[std::ops::Range<usize>]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    // Ranges come from matching an ASCII `@name`, so they fall on char boundaries.
    for range in ranges {
        out.push_str(text.get(last..range.start).unwrap_or_default());
        last = range.end;
    }
    out.push_str(text.get(last..).unwrap_or_default());
    out.trim().to_string()
}

/// The text to hand the agent if a group message is addressed to the bot,
/// with the bot's own mentions removed; `None` if it is not for the bot.
pub(super) fn addressed_text(text: &str, username: &str, replies_to_bot: bool) -> Option<String> {
    let mentions = mention_ranges(text, username);
    if let Some(command) = text.strip_prefix('/') {
        let name = command.split_whitespace().next().unwrap_or_default();
        return match name.split_once('@') {
            Some((_, target)) if !target.eq_ignore_ascii_case(username) => None,
            _ => Some(without_ranges(text, &mentions)),
        };
    }
    if mentions.is_empty() && !replies_to_bot {
        return None;
    }
    Some(without_ranges(text, &mentions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_is_required_and_stripped() {
        assert_eq!(
            addressed_text("@ResiBot can you check the build?", "resibot", false).as_deref(),
            Some("can you check the build?")
        );
        assert_eq!(addressed_text("check the build", "resibot", false), None);
    }

    #[test]
    fn a_longer_name_is_not_a_mention() {
        assert_eq!(addressed_text("@resibot2 hi", "resibot", false), None);
    }

    #[test]
    fn replies_to_the_bot_count_without_a_mention() {
        assert_eq!(
            addressed_text("yes, do that", "resibot", true).as_deref(),
            Some("yes, do that")
        );
    }

    #[test]
    fn commands_for_other_bots_are_ignored() {
        assert_eq!(
            addressed_text("/status@resibot", "resibot", false).as_deref(),
            Some("/status")
        );
        assert_eq!(
            addressed_text("/status", "resibot", false).as_deref(),
            Some("/status")
        );
        assert_eq!(addressed_text("/status@otherbot", "resibot", false), None);
    }

    #[test]
    fn group_label_uses_the_title() {
        assert_eq!(group_label(Some("Launch prep")), "group \"Launch prep\"");
        assert_eq!(group_label(None), "group");
    }
}
